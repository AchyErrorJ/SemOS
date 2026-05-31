// M27 Phase 2a A1 + R4 B2: jobserver. Upstream wraps the
// jobserver crate behind a LazyLock-initialized global plus a Proxy
// struct that talks to a HelperThread. On SemOS there is no jobserver
// IPC channel and §1.4 makes the compile single-threaded, so the
// public API is stubbed to a no-op Client and a no-op Proxy.
//
// Host build path is unchanged.

#[cfg(not(target_os = "none"))]
mod imp_std {
    use std::sync::{Arc, LazyLock, OnceLock};

    pub use jobserver_crate::{Acquired, Client, HelperThread};
    use jobserver_crate::{FromEnv, FromEnvErrorKind};
    use parking_lot::{Condvar, Mutex};

    // We can only call `from_env_ext` once per process

    // We stick this in a global because there could be multiple rustc instances
    // in this process, and the jobserver is per-process.
    static GLOBAL_CLIENT: LazyLock<Result<Client, String>> = LazyLock::new(|| {
        let FromEnv { client, var } = unsafe { Client::from_env_ext(true) };

        let error = match client {
            Ok(client) => return Ok(client),
            Err(e) => e,
        };

        if matches!(
            error.kind(),
            FromEnvErrorKind::NoEnvVar
                | FromEnvErrorKind::NoJobserver
                | FromEnvErrorKind::NegativeFd
                | FromEnvErrorKind::Unsupported
        ) {
            return Ok(default_client());
        }

        let (name, value) = var.unwrap();
        Err(format!(
            "failed to connect to jobserver from environment variable `{name}={:?}`: {error}",
            value
        ))
    });

    fn default_client() -> Client {
        let client = Client::new(32).expect("failed to create jobserver");
        client.acquire_raw().ok();
        client
    }

    static GLOBAL_CLIENT_CHECKED: OnceLock<Client> = OnceLock::new();

    pub fn initialize_checked(report_warning: impl FnOnce(&'static str)) {
        let client_checked = match &*GLOBAL_CLIENT {
            Ok(client) => client.clone(),
            Err(e) => {
                report_warning(e);
                default_client()
            }
        };
        GLOBAL_CLIENT_CHECKED.set(client_checked).ok();
    }

    const ACCESS_ERROR: &str = "jobserver check should have been called earlier";

    pub fn client() -> Client {
        GLOBAL_CLIENT_CHECKED.get().expect(ACCESS_ERROR).clone()
    }

    struct ProxyData {
        used: u16,
        pending: u16,
    }

    pub struct Proxy {
        client: Client,
        data: Mutex<ProxyData>,
        wake_pending: Condvar,
        helper: OnceLock<HelperThread>,
    }

    impl Proxy {
        pub fn new() -> Arc<Self> {
            let proxy = Arc::new(Proxy {
                client: client(),
                data: Mutex::new(ProxyData { used: 1, pending: 0 }),
                wake_pending: Condvar::new(),
                helper: OnceLock::new(),
            });
            let proxy_ = Arc::clone(&proxy);
            let helper = proxy
                .client
                .clone()
                .into_helper_thread(move |token| {
                    if let Ok(token) = token {
                        let mut data = proxy_.data.lock();
                        if data.pending > 0 {
                            token.drop_without_releasing();
                            assert!(data.used > 0);
                            data.used += 1;
                            data.pending -= 1;
                            proxy_.wake_pending.notify_one();
                        } else {
                            drop(data);
                            drop(token);
                        }
                    }
                })
                .expect("failed to create helper thread");
            proxy.helper.set(helper).unwrap();
            proxy
        }

        pub fn acquire_thread(&self) {
            let mut data = self.data.lock();

            if data.used == 0 {
                assert_eq!(data.pending, 0);
                data.used += 1;
            } else {
                self.helper.get().unwrap().request_token();
                data.pending += 1;
                self.wake_pending.wait(&mut data);
            }
        }

        pub fn release_thread(&self) {
            let mut data = self.data.lock();

            if data.pending > 0 {
                data.pending -= 1;
                self.wake_pending.notify_one();
            } else {
                data.used -= 1;

                if data.used > 0 {
                    drop(data);
                    self.client.release_raw().ok();
                }
            }
        }
    }
}

#[cfg(target_os = "none")]
mod imp_none {
    use alloc::sync::Arc;

    /// M27 R4 B2: stub Client. No jobserver IPC on SemOS; tokens are
    /// freely available since the compile is single-threaded.
    #[derive(Clone, Debug, Default)]
    pub struct Client;

    impl Client {
        pub fn acquire(&self) -> core::result::Result<Acquired, AcquireError> {
            Ok(Acquired)
        }
        pub fn release_raw(&self) -> core::result::Result<(), AcquireError> {
            Ok(())
        }
    }

    /// Stub token; dropping releases nothing (no kernel-side state).
    #[derive(Debug)]
    pub struct Acquired;
    impl Acquired {
        pub fn drop_without_releasing(self) {}
    }

    #[derive(Debug)]
    pub struct AcquireError;

    /// Stub helper thread; on SemOS there's no separate worker so
    /// `request_token` is a no-op.
    #[derive(Debug)]
    pub struct HelperThread;
    impl HelperThread {
        pub fn request_token(&self) {}
    }

    pub fn initialize_checked(_report_warning: impl FnOnce(&'static str)) {
        // No-op on SemOS.
    }

    pub fn client() -> Client {
        Client
    }

    /// Stub jobserver proxy — `acquire_thread`/`release_thread` are
    /// no-ops because there is only one rustc worker.
    pub struct Proxy;

    impl Proxy {
        pub fn new() -> Arc<Self> {
            Arc::new(Proxy)
        }
        pub fn acquire_thread(&self) {}
        pub fn release_thread(&self) {}
    }
}

#[cfg(not(target_os = "none"))]
pub use imp_std::*;
#[cfg(target_os = "none")]
pub use imp_none::*;
