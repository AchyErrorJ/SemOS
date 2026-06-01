// Stage F1: cfg-split between host (parking_lot + thread_local
// machinery for true per-thread storage) and SemOS (single-threaded
// per §1.4 — WorkerLocal collapses to a single CacheAligned<T> and
// Registry is a unit-like tag for source compatibility).

#[cfg(not(target_os = "none"))]
mod host {
    use alloc::boxed::Box;
    use alloc::sync::Arc;
    use core::cell::{Cell, OnceCell};
    use core::num::NonZero;
    use core::ops::Deref;
    use core::ptr;

    use parking_lot::Mutex;

    use crate::outline;
    use crate::sync::CacheAligned;

    /// A pointer to the `RegistryData` which uniquely identifies a registry.
    /// This identifier can be reused if the registry gets freed.
    #[derive(Clone, Copy, PartialEq)]
    struct RegistryId(*const RegistryData);

    impl RegistryId {
        #[inline(always)]
        /// Verifies that the current thread is associated with the registry and returns its unique
        /// index within the registry. This panics if the current thread is not associated with this
        /// registry.
        fn verify(self) -> usize {
            let (id, index) = THREAD_DATA.with(|data| (data.registry_id.get(), data.index.get()));

            if id == self { index } else { outline(|| panic!("Unable to verify registry association")) }
        }
    }

    struct RegistryData {
        thread_limit: NonZero<usize>,
        threads: Mutex<usize>,
    }

    /// Represents a list of threads which can access worker locals.
    #[derive(Clone)]
    pub struct Registry(Arc<RegistryData>);

    std::thread_local! {
        /// The registry associated with the thread.
        static REGISTRY: OnceCell<Registry> = const { OnceCell::new() };
    }

    struct ThreadData {
        registry_id: Cell<RegistryId>,
        index: Cell<usize>,
    }

    std::thread_local! {
        /// A thread local which contains the identifier of `REGISTRY` but allows for faster access.
        static THREAD_DATA: ThreadData = const { ThreadData {
            registry_id: Cell::new(RegistryId(ptr::null())),
            index: Cell::new(0),
        }};
    }

    impl Registry {
        pub fn new(thread_limit: NonZero<usize>) -> Self {
            Registry(Arc::new(RegistryData { thread_limit, threads: Mutex::new(0) }))
        }

        pub fn current() -> Self {
            REGISTRY.with(|registry| registry.get().cloned().expect("No associated registry"))
        }

        pub fn register(&self) {
            let mut threads = self.0.threads.lock();
            if *threads < self.0.thread_limit.get() {
                REGISTRY.with(|registry| {
                    if registry.get().is_some() {
                        drop(threads);
                        panic!("Thread already has a registry");
                    }
                    registry.set(self.clone()).ok();
                    THREAD_DATA.with(|data| {
                        data.registry_id.set(self.id());
                        data.index.set(*threads);
                    });
                    *threads += 1;
                });
            } else {
                drop(threads);
                panic!("Thread limit reached");
            }
        }

        fn id(&self) -> RegistryId {
            RegistryId(&*self.0)
        }
    }

    /// Holds worker local values for each possible thread in a registry.
    pub struct WorkerLocal<T> {
        locals: Box<[CacheAligned<T>]>,
        registry: Registry,
    }

    unsafe impl<T: Send> Sync for WorkerLocal<T> {}

    impl<T> WorkerLocal<T> {
        #[inline]
        pub fn new<F: FnMut(usize) -> T>(mut initial: F) -> WorkerLocal<T> {
            let registry = Registry::current();
            WorkerLocal {
                locals: (0..registry.0.thread_limit.get()).map(|i| CacheAligned(initial(i))).collect(),
                registry,
            }
        }

        #[inline]
        pub fn into_inner(self) -> impl Iterator<Item = T> {
            self.locals.into_vec().into_iter().map(|local| local.0)
        }
    }

    impl<T> Deref for WorkerLocal<T> {
        type Target = T;

        #[inline(always)]
        fn deref(&self) -> &T {
            unsafe { &self.locals.get_unchecked(self.registry.id().verify()).0 }
        }
    }

    impl<T: Default> Default for WorkerLocal<T> {
        fn default() -> Self {
            WorkerLocal::new(|_| T::default())
        }
    }
}

#[cfg(target_os = "none")]
mod target {
    // Single-threaded SemOS stub. §1.4 drops rayon so there's no
    // notion of multiple worker threads — every WorkerLocal degenerates
    // to a single T owned by the (process-wide, only) "registry".
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use core::num::NonZero;
    use core::ops::Deref;

    use crate::sync::CacheAligned;

    /// Single-threaded stand-in. `thread_limit` is recorded but
    /// effectively unused since there's only one worker.
    #[derive(Clone)]
    pub struct Registry {
        // Phantom field — kept so the source-level shape matches and
        // any caller that passes through `Registry::new(n)` still
        // works for n >= 1.
        thread_limit: NonZero<usize>,
    }

    impl Registry {
        pub fn new(thread_limit: NonZero<usize>) -> Self {
            Self { thread_limit }
        }

        pub fn current() -> Self {
            // Only one thread ever runs, so "the current registry" is
            // always a fresh one with thread_limit=1.
            Self::new(unsafe { NonZero::new_unchecked(1) })
        }

        pub fn register(&self) {
            // No-op: registration is meaningless without multiple threads.
        }
    }

    pub struct WorkerLocal<T> {
        local: Box<CacheAligned<T>>,
    }

    // Single-threaded — `Sync` is trivially safe (no other thread can
    // observe the value).
    unsafe impl<T: Send> Sync for WorkerLocal<T> {}

    impl<T> WorkerLocal<T> {
        #[inline]
        pub fn new<F: FnMut(usize) -> T>(mut initial: F) -> WorkerLocal<T> {
            WorkerLocal { local: Box::new(CacheAligned(initial(0))) }
        }

        #[inline]
        pub fn into_inner(self) -> impl Iterator<Item = T> {
            let mut v: Vec<T> = Vec::with_capacity(1);
            v.push(self.local.0);
            v.into_iter()
        }
    }

    impl<T> Deref for WorkerLocal<T> {
        type Target = T;

        #[inline(always)]
        fn deref(&self) -> &T {
            &self.local.0
        }
    }

    impl<T: Default> Default for WorkerLocal<T> {
        fn default() -> Self {
            WorkerLocal::new(|_| T::default())
        }
    }
}

#[cfg(not(target_os = "none"))]
pub use host::{Registry, WorkerLocal};
#[cfg(target_os = "none")]
pub use target::{Registry, WorkerLocal};
