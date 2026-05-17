//! `NetworkTransport` backed by `TcpStream` + embedded-tls's TLS 1.3
//! client. End of the line for Phase 8: with this wired into
//! [`crate::llm::net_provider::NetworkLlmProvider`], the kernel can
//! POST to `api.anthropic.com/v1/messages` over real TLS.
//!
//! # Stack from bottom to top
//!
//! ```text
//!  +---------------------------------------------------------------+
//!  |  NetworkLlmProvider (HTTP framing + JSON)                     |
//!  +---------------------------------------------------------------+
//!  |  NetworkTransport trait (connect/send/recv/close)             |
//!  +---------------------------------------------------------------+
//!  |  TlsTransport (THIS FILE)                                     |
//!  |    holds: TlsConnection<TcpStream, Chacha20Poly1305Sha256>    |
//!  +---------------------------------------------------------------+
//!  |  embedded-tls 0.18 blocking::TlsConnection                    |
//!  |    handshake | key-schedule | record layer                    |
//!  |    crypto: KernelChacha20Poly1305 + KernelSha256              |
//!  |    verifier: SpkiPinVerifier (pins WE1 intermediate)          |
//!  |    rng: KernelRng (RDRAND-backed)                             |
//!  +---------------------------------------------------------------+
//!  |  embedded_io::{Read, Write} on TcpStream                      |
//!  +---------------------------------------------------------------+
//!  |  smoltcp tcp::Socket (managed in kernel-core::net)            |
//!  +---------------------------------------------------------------+
//!  |  virtio-net driver (kernel-x86_64)                            |
//!  +---------------------------------------------------------------+
//! ```
//!
//! # Configuration
//!
//! TlsTransport keeps a fixed remote IP/port + SNI hostname configured
//! at runtime via [`TlsTransport::set_remote_endpoint`]. We don't have
//! DNS yet, so the caller resolves separately and hands us both pieces.
//! The hostname is used purely as the SNI extension; the actual TCP
//! connect uses the IP. The SPKI pin handles the trust decision — host
//! name validation would be redundant, see [`super::verifier`].
//!
//! # What this module does NOT do
//!
//! - **DNS** — there's no resolver; caller supplies the IP.
//! - **Connection reuse / TLS session resumption** — every `connect()`
//!   does a fresh full handshake. Cheap to add later via embedded-tls's
//!   session ticket support if we ever need throughput.
//! - **Concurrent connections** — one TLS session in flight per kernel,
//!   matching the existing TcpStream single-socket invariant.

use embedded_tls::{
    Certificate, CryptoProvider, NoSign, TlsConfig, TlsContext, TlsError,
    TlsVerifier,
};
use embedded_tls::blocking::TlsConnection;
use rand_core::{CryptoRng, Error as RngError, RngCore};

use crate::llm::transport::{NetworkTransport, TransportError, MAX_HOST_LEN};
use crate::net::{self, Ipv4Address, TcpStream, TcpState};
use crate::platform;
use crate::tls::cipher_suite::Chacha20Poly1305Sha256;
use crate::tls::verifier::SpkiPinVerifier;

// ============================================================================
// KernelRng — RDRAND-backed CryptoRngCore
// ============================================================================

/// `rand_core::RngCore` + `CryptoRng` implementation backed by the
/// platform's hardware RNG (RDRAND on x86_64).
///
/// `Platform::random_bytes` returns `Result<(), ()>` because the
/// trait is abstract over implementations that might fail. On x86_64
/// RDRAND can in principle fail to return entropy after retries; on
/// real hardware we'd handle that. For now we panic — TLS that
/// silently uses zero-bytes for nonces is catastrophically worse
/// than refusing to boot, and the boot-time RNG probe already
/// guarantees RDRAND is present.
pub struct KernelRng;

impl RngCore for KernelRng {
    fn next_u32(&mut self) -> u32 {
        let mut buf = [0u8; 4];
        self.fill_bytes(&mut buf);
        u32::from_le_bytes(buf)
    }
    fn next_u64(&mut self) -> u64 {
        let mut buf = [0u8; 8];
        self.fill_bytes(&mut buf);
        u64::from_le_bytes(buf)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        platform::random_bytes(dest).expect("kernel RNG failed");
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), RngError> {
        // rand_core 0.6's Error is opaque; we just signal failure.
        // The construction path is `Error::from(NonZeroU32)`, which
        // requires picking an arbitrary code — use 1 (UNKNOWN).
        platform::random_bytes(dest)
            .map_err(|_| RngError::from(core::num::NonZeroU32::new(1).unwrap()))
    }
}

impl CryptoRng for KernelRng {}

// ============================================================================
// KernelCryptoProvider — wires our verifier + RNG into embedded-tls
// ============================================================================

/// `CryptoProvider` impl carrying everything embedded-tls needs from
/// us at handshake time: an RNG for fresh client_random + key-share
/// scalars, and the SPKI-pinning verifier for cert/signature checks.
///
/// `Signature` is set to `p256::ecdsa::DerSignature` because the trait
/// requires it and that type is already in embedded-tls's dependency
/// tree (via the `p256` crate). The signer path is unimplemented — we
/// never present a client certificate.
pub struct KernelCryptoProvider {
    rng: KernelRng,
    verifier: SpkiPinVerifier,
}

impl KernelCryptoProvider {
    pub fn new() -> Self {
        Self { rng: KernelRng, verifier: SpkiPinVerifier::new() }
    }
}

impl Default for KernelCryptoProvider {
    fn default() -> Self { Self::new() }
}

impl CryptoProvider for KernelCryptoProvider {
    type CipherSuite = Chacha20Poly1305Sha256;
    type Signature = p256::ecdsa::DerSignature;

    fn rng(&mut self) -> impl rand_core::CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
        Ok(&mut self.verifier)
    }

    // We don't implement `signer` — the default returns Unimplemented,
    // which is what we want. embedded-tls only calls it when a server
    // requests client authentication; Anthropic doesn't.
}

// ============================================================================
// Static TLS record buffers
// ============================================================================
//
// embedded-tls needs read + write record buffers sized to the maximum
// TLS record (16 KiB + 5 header + 256 cipher overhead ≈ 16.5 KiB).
// Round up to 17 KiB. These are the largest static buffers in
// kernel-core — be conservative if cutting memory pressure.

const TLS_RECORD_BUF_SIZE: usize = 17 * 1024;

static mut TLS_RX_BUF: [u8; TLS_RECORD_BUF_SIZE] = [0; TLS_RECORD_BUF_SIZE];
static mut TLS_TX_BUF: [u8; TLS_RECORD_BUF_SIZE] = [0; TLS_RECORD_BUF_SIZE];

/// Have the buffers been claimed by an active TlsTransport? Mirrors
/// the `SOCKET_IN_USE` discipline in `net::tcp`. Single connection at
/// a time matches our single-TcpStream invariant.
static mut TLS_BUFFERS_IN_USE: bool = false;

// ============================================================================
// TlsTransport
// ============================================================================

/// Max length of the SNI hostname we'll accept. Plenty for typical API
/// endpoints; matches [`MAX_HOST_LEN`] for symmetry with the loopback.
pub const MAX_SNI_LEN: usize = MAX_HOST_LEN;

/// Wall-clock budget for TCP connect to reach a terminal state, in
/// units of platform ticks. The APIC timer is 100 Hz so 1000 ticks
/// = 10 seconds. SLIRP→host→internet round-trips can take 50-200 ms,
/// with retransmit backoff if the first SYN is lost; 10 s is a
/// generous outer bound that still lets us bail before the kernel
/// looks hung.
const TCP_CONNECT_TIMEOUT_TICKS: u64 = 1000;

/// TLS 1.3 client transport wrapping a TcpStream.
///
/// One outbound TLS connection at a time; matches the kernel's
/// single-socket invariant. Lifetime parameters are erased: the
/// underlying `TlsConnection<'_, TcpStream, …>` borrows our static
/// buffers, so the connection itself can live in this struct without
/// a lifetime escaping.
pub struct TlsTransport {
    conn: Option<TlsConnection<'static, TcpStream, Chacha20Poly1305Sha256>>,
    /// IP we connect to. Set via [`set_remote_endpoint`]; without it
    /// `connect()` returns `InvalidHost` because we can't do DNS yet.
    remote_ip: Option<Ipv4Address>,
    /// Port to connect to. Always 443 for TLS, but stored separately
    /// so a future debug-only TLS-on-nonstandard-port test can override.
    remote_port: u16,
    /// SNI hostname buffer. The TlsConfig API takes `&'static str`,
    /// so we leak this into a static slot — see [`SNI_BUF`].
    sni_len: usize,
    connected: bool,
}

/// Backing storage for the SNI string we hand to `TlsConfig::with_server_name`.
/// embedded-tls keeps the `&'static str` for the connection's lifetime;
/// we need somewhere stable to point it.
static mut SNI_BUF: [u8; MAX_SNI_LEN] = [0; MAX_SNI_LEN];

impl TlsTransport {
    /// Create an unconfigured transport. Call [`set_remote_endpoint`]
    /// before [`NetworkTransport::connect`] to give it an IP.
    pub const fn new() -> Self {
        Self {
            conn: None,
            remote_ip: None,
            remote_port: 443,
            sni_len: 0,
            connected: false,
        }
    }

    /// Configure where this transport connects. Required before
    /// [`NetworkTransport::connect`] — without DNS the kernel can't
    /// derive `ip` from the host string.
    ///
    /// The host string passed at connect time becomes the SNI; this
    /// `ip:port` is the actual TCP target.
    pub fn set_remote_endpoint(&mut self, ip: Ipv4Address, port: u16) {
        self.remote_ip = Some(ip);
        self.remote_port = port;
    }

    /// Return the configured remote IP, if any. Useful for diagnostics.
    pub fn remote_ip(&self) -> Option<Ipv4Address> { self.remote_ip }

    /// Return the embedded-tls error from the most recent failed
    /// handshake on the global singleton, if any. `None` until at
    /// least one handshake fails. Diagnostic-only — the
    /// NetworkTransport surface collapses everything to
    /// `TransportError::Closed`, which loses the cause.
    pub fn last_handshake_error() -> Option<TlsError> {
        unsafe { LAST_HANDSHAKE_ERROR }
    }

    /// Last TCP-level state reached during connect(). `None` until at
    /// least one connect attempt has been made. `Some(Established)`
    /// means TCP came up and any failure that follows is at the TLS
    /// layer (read [`Self::last_handshake_error`]).
    pub fn last_tcp_state() -> Option<TcpState> {
        unsafe { LAST_TCP_STATE }
    }

    /// Last `TlsError` from a post-handshake send/recv on the global
    /// singleton, if any. Diagnostic-only.
    pub fn last_io_error() -> Option<TlsError> {
        unsafe { LAST_IO_ERROR }
    }

    /// Drive the TCP socket from SYN-SENT to a terminal state.
    /// Returns the final state; caller decides what to do with it.
    ///
    /// Loop discipline: keep polling smoltcp + the platform tick
    /// counter until either the socket reaches a terminal state or
    /// `TCP_CONNECT_TIMEOUT_TICKS` of wall-clock time elapses.
    /// `core::hint::spin_loop()` between polls lets the CPU back
    /// off cleanly so the APIC timer interrupt and the virtio-net
    /// RX interrupt have a chance to land.
    fn poll_to_terminal(&self, stream: &TcpStream) -> TcpState {
        let start = crate::platform::ticks();
        loop {
            net::poll();
            let s = stream.state();
            match s {
                TcpState::Established | TcpState::Closed => return s,
                _ => {} // still SynSent / SynReceived / etc.
            }
            let elapsed = crate::platform::ticks().wrapping_sub(start);
            if elapsed >= TCP_CONNECT_TIMEOUT_TICKS { return stream.state(); }
            core::hint::spin_loop();
        }
    }
}

impl Default for TlsTransport {
    fn default() -> Self { Self::new() }
}

// ----------------------------------------------------------------------------
// NetworkTransport impl
// ----------------------------------------------------------------------------

impl NetworkTransport for TlsTransport {
    fn connect(&mut self, host: &str, port: u16) -> Result<(), TransportError> {
        if self.connected {
            // Tear down the old session first — single-connection invariant.
            self.close();
        }
        if host.is_empty() || host.len() > MAX_SNI_LEN {
            return Err(TransportError::InvalidHost);
        }

        let remote_ip = self.remote_ip.ok_or(TransportError::InvalidHost)?;
        // Caller can override port via the NetworkTransport::connect
        // arg, but most callers will leave it at our configured 443.
        let remote_port = if port == 0 { self.remote_port } else { port };

        // Claim the static record buffers. If something else is already
        // using them (shouldn't happen — we tear down on close — but
        // be loud about it) bail.
        unsafe {
            if TLS_BUFFERS_IN_USE {
                return Err(TransportError::NoBuffers);
            }
        }

        // Stage 1: open TCP socket, wait for SYN/SYN-ACK/ACK to complete.
        let mut stream = TcpStream::connect(remote_ip, remote_port)
            .map_err(|_| {
                unsafe { LAST_TCP_STATE = Some(TcpState::Closed); }
                TransportError::Io
            })?;

        let final_state = self.poll_to_terminal(&stream);
        unsafe { LAST_TCP_STATE = Some(final_state); }
        if final_state != TcpState::Established {
            // Couldn't reach the peer (RST, timeout, gateway dropped).
            // Release the socket so the next try gets a fresh slot.
            let _ = stream;
            return Err(TransportError::Closed);
        }

        // Stage 2: stash the SNI in our static slot so embedded-tls
        // can hold a 'static reference. `set_server_name` takes &str,
        // so we go through from_utf8 on the slice we just wrote.
        let sni_str: &'static str = unsafe {
            let buf = &mut *core::ptr::addr_of_mut!(SNI_BUF);
            buf[..host.len()].copy_from_slice(host.as_bytes());
            self.sni_len = host.len();
            // We just wrote valid UTF-8 (came from `host: &str`); skip
            // re-validation. The slice we expose is exactly what we wrote.
            core::str::from_utf8_unchecked(&buf[..host.len()])
        };

        // Stage 3: build the TLS config + context, then run open().
        // The handshake runs synchronously inside open() — sends
        // ClientHello, reads ServerHello, exchanges keys, validates
        // cert chain via our SpkiPinVerifier, finishes.
        let config: TlsConfig<'static> = TlsConfig::new()
            .with_server_name(sni_str)
            // We don't supply a CA cert — the SPKI pin replaces it.
            // `with_cert` would be for client auth; not used.
            ;

        let rx_buf: &'static mut [u8] = unsafe {
            &mut *core::ptr::addr_of_mut!(TLS_RX_BUF)
        };
        let tx_buf: &'static mut [u8] = unsafe {
            &mut *core::ptr::addr_of_mut!(TLS_TX_BUF)
        };

        let mut conn: TlsConnection<'static, TcpStream, Chacha20Poly1305Sha256> =
            TlsConnection::new(stream, rx_buf, tx_buf);

        // Note: TlsConfig must live as long as the open() call. We hold
        // it on the stack here; that's fine because open() borrows it
        // and returns before we leave the function.
        let context = TlsContext::new(&config, KernelCryptoProvider::new());

        match conn.open(context) {
            Ok(()) => {
                self.conn = Some(conn);
                self.connected = true;
                unsafe { TLS_BUFFERS_IN_USE = true; }
                Ok(())
            }
            Err(e) => {
                // Handshake failure (bad cert, pin mismatch, network
                // tear-down mid-handshake). The connection drops here;
                // TcpStream's Drop releases the socket.
                // Stash the last error so callers can diagnose;
                // NetworkTransport's enum is too small for the detail.
                unsafe { LAST_HANDSHAKE_ERROR = Some(e); }
                Err(TransportError::Closed)
            }
        }
    }

    fn send(&mut self, data: &[u8]) -> Result<usize, TransportError> {
        let conn = self.conn.as_mut().ok_or(TransportError::InvalidState)?;
        conn.write(data).map_err(|e| {
            unsafe { LAST_IO_ERROR = Some(e); }
            TransportError::Io
        })
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let conn = self.conn.as_mut().ok_or(TransportError::InvalidState)?;
        // Flush any pending write record before reading — otherwise the
        // peer never sees what we wrote and we deadlock waiting for a
        // reply. embedded-tls's write() only buffers; recv() is the
        // natural "I'm done sending, your turn" sync point for an HTTP
        // request/response flow.
        if let Err(e) = conn.flush() {
            unsafe { LAST_IO_ERROR = Some(e); }
            return Err(TransportError::Io);
        }
        conn.read(buf).map_err(|e| {
            unsafe { LAST_IO_ERROR = Some(e); }
            TransportError::Io
        })
    }

    fn close(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            // Send a close_notify alert; ignore errors (we're tearing
            // down anyway). embedded-tls's close() takes the underlying
            // socket back out via destruct, but we don't need the
            // socket — let Drop release it.
            let _ = conn.close();
        }
        self.connected = false;
        unsafe { TLS_BUFFERS_IN_USE = false; }
    }

    fn is_connected(&self) -> bool { self.connected }
    fn name(&self) -> &'static str { "tls-tcp" }
}

// ============================================================================
// Global singleton
// ============================================================================
//
// Mirrors the global_loopback_transport pattern in llm::transport so
// NetworkLlmProvider can route to whichever transport its Endpoint
// selects without juggling owned references.

static mut GLOBAL_TLS: TlsTransport = TlsTransport::new();

/// Last embedded-tls handshake error, populated when `connect()` fails
/// inside `TlsConnection::open`. Read via [`TlsTransport::last_handshake_error`].
static mut LAST_HANDSHAKE_ERROR: Option<TlsError> = None;

/// Final TCP state observed during the most recent `connect()`. Helps
/// distinguish "TCP never came up" from "TLS handshake failed."
static mut LAST_TCP_STATE: Option<TcpState> = None;

/// Last `TlsError` from a failed `send`/`recv` after the handshake
/// completed. Diagnostic-only — the NetworkTransport surface collapses
/// every error to `TransportError::Io`, which loses the cause.
static mut LAST_IO_ERROR: Option<TlsError> = None;

/// Get the global TLS transport. Used by `NetworkLlmProvider` when
/// the configured `TransportKind` is `TlsTcp`.
///
/// # Safety
/// Single-threaded kernel; same soft-serialised contract as the
/// other global transports.
pub unsafe fn global_tls_transport() -> &'static mut TlsTransport {
    &mut *core::ptr::addr_of_mut!(GLOBAL_TLS)
}

/// Convenience used at boot to point the TLS transport at a fixed IP
/// before the LLM provider tries to use it. Until DNS exists this
/// must be called before any TLS connection is attempted.
pub fn configure_global(ip: Ipv4Address, port: u16) {
    unsafe { global_tls_transport().set_remote_endpoint(ip, port); }
}
