//! Threading — `std::thread`-shaped subset (M25 #52 adds spawn/join).
//!
//! `spawn(closure) -> JoinHandle<T>` boxes the closure + a result slot
//! on the shared heap, hands the box pointer to SYS_THREAD_SPAWN as the
//! thread arg, and the new thread runs the closure, stores its result,
//! and exits. `JoinHandle::join` blocks via SYS_THREAD_JOIN and reads
//! the result back.
//!
//! Threads share the spawning process's address space (same CR3, same
//! heap mapping), so the boxed payload is reachable from both sides.

use core::cell::UnsafeCell;
use core_alloc::boxed::Box;
use crate::arch::{SYS_SLEEP, SYS_THREAD_SPAWN, SYS_THREAD_JOIN, syscall1, syscall2};

/// Block the current task for `ticks` scheduler timer ticks.
///
/// Convert from a Duration using `kernel-core::scheduler::
/// SCHEDULER_TICK_HZ` (≈62 Hz). [`sleep_ms`] does the common case.
pub fn sleep_ticks(ticks: u64) {
    unsafe {
        let _ = syscall1(SYS_SLEEP, ticks);
    }
}

/// Sleep for approximately `ms` milliseconds (best-effort; rounds up to
/// at least one tick). Tick rate ≈ 62 Hz → ~16 ms per tick.
pub fn sleep_ms(ms: u64) {
    // ticks = ceil(ms * HZ / 1000), HZ = 62.
    let ticks = (ms * 62 + 999) / 1000;
    sleep_ticks(ticks.max(1));
}

/// Low-level: spawn a raw `extern "C" fn(u64) -> !` thread with one
/// u64 argument. Returns the tid (slot) or u64::MAX. Prefer [`spawn`].
pub fn spawn_raw(entry: extern "C" fn(u64) -> !, arg: u64) -> u64 {
    unsafe { syscall2(SYS_THREAD_SPAWN, entry as u64, arg) }
}

/// Low-level: join a raw tid; returns its exit code.
pub fn join_raw(tid: u64) -> u64 {
    unsafe { syscall1(SYS_THREAD_JOIN, tid) }
}

/// Heap payload shared between spawner and child thread. The closure is
/// taken (consumed) by the child; the result is written by the child
/// and read by `join`.
struct Payload<F, T> {
    closure: Option<F>,
    result: UnsafeCell<Option<T>>,
}

/// Monomorphized thread entry. The kernel delivers `arg` (the boxed
/// payload pointer) in rdi per SysV. We run the closure, stash the
/// result, then exit — which the kernel observes as TaskState::Exited,
/// unblocking the joiner.
extern "C" fn thread_entry<F, T>(arg: u64) -> !
where
    F: FnOnce() -> T,
{
    let payload = arg as *mut Payload<F, T>;
    unsafe {
        let f = (*payload).closure.take().expect("thread closure already taken");
        let result = f();
        *(*payload).result.get() = Some(result);
    }
    // Exit code 0 — join reads the actual T from the payload, not the code.
    crate::process::exit(0);
}

/// Handle to a spawned thread. `join()` waits for it and returns its
/// result. Dropping without joining leaks the thread's payload (same
/// as detaching) — a `Drop`-detach refinement can come later.
pub struct JoinHandle<T> {
    tid: u64,
    payload: *mut (),
    join_fn: unsafe fn(*mut ()) -> Option<T>,
    tid_valid: bool,
}

// SAFETY: T: Send is required at spawn; the payload pointer is only
// dereferenced after the child has exited (join), so no aliasing.
unsafe impl<T: Send> Send for JoinHandle<T> {}

impl<T> JoinHandle<T> {
    /// The scheduler tid (slot) backing this thread.
    pub fn tid(&self) -> u64 {
        self.tid
    }

    /// Block until the thread finishes; returns its result. `Err(())`
    /// if the spawn failed or the result was already taken.
    pub fn join(mut self) -> Result<T, ()> {
        if !self.tid_valid {
            return Err(());
        }
        let _code = join_raw(self.tid);
        self.tid_valid = false;
        // SAFETY: the child has exited (join_raw blocked until Exited),
        // so it's done writing the result and we have exclusive access.
        unsafe { (self.join_fn)(self.payload).ok_or(()) }
    }
}

/// Type-erased result extractor + payload free. Stored in JoinHandle so
/// `join` doesn't need to be generic over the closure type F.
unsafe fn collect_payload<F, T>(payload: *mut ()) -> Option<T> {
    let payload = payload as *mut Payload<F, T>;
    let result = (*(*payload).result.get()).take();
    // Reclaim the heap box.
    drop(Box::from_raw(payload));
    result
}

/// Spawn a thread running `f`, returning a `JoinHandle<T>`.
///
/// `F: Send + 'static`, `T: Send + 'static` — same bounds as
/// `std::thread::spawn`. On spawn failure the handle's `join` returns
/// `Err(())`.
pub fn spawn<F, T>(f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let payload = Box::into_raw(Box::new(Payload::<F, T> {
        closure: Some(f),
        result: UnsafeCell::new(None),
    }));
    let entry: extern "C" fn(u64) -> ! = thread_entry::<F, T>;
    let tid = spawn_raw(entry, payload as u64);
    if tid == u64::MAX {
        // Spawn failed — reclaim the box so we don't leak, and hand back
        // a handle whose join() reports the error.
        // SAFETY: nothing else holds this pointer (the thread never ran).
        unsafe {
            drop(Box::from_raw(payload));
        }
        return JoinHandle {
            tid: u64::MAX,
            payload: core::ptr::null_mut(),
            join_fn: collect_payload::<F, T>,
            tid_valid: false,
        };
    }
    JoinHandle {
        tid,
        payload: payload as *mut (),
        join_fn: collect_payload::<F, T>,
        tid_valid: true,
    }
}

// ---------------------------------------------------------------------
// LocalKey<T> + thread_local! macro — M27 R4 B2.
//
// SemOS Ring-3 processes are single-threaded today (futures over
// SYS_THREAD_SPAWN don't get parallelism — they multiplex on the
// single hardware context the kernel hands a user process). For the
// rustc port that's fine: the rustc model assumes thread-local
// storage but we can collapse it to "lazy-init a single process-wide
// cell". If/when we add real multi-threading, replace this LocalKey
// with one that indexes per-tid.
// ---------------------------------------------------------------------

/// `std::thread::LocalKey`-shaped thread-local accessor. On SemOS
/// (single-threaded Ring 3), this is a process-wide lazy cell — the
/// init function runs the first time `.with()` is called and the
/// value persists for the rest of the process.
pub struct LocalKey<T: 'static> {
    inner: crate::sync::OnceLock<T>,
    init: fn() -> T,
}

impl<T: 'static> LocalKey<T> {
    /// Build a LocalKey from an init function. Usually invoked by the
    /// `thread_local!` macro rather than directly.
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            inner: crate::sync::OnceLock::new(),
            init,
        }
    }

    /// Run `f` with a reference to the thread-local value, initializing
    /// it if this is the first access.
    pub fn with<R, F: FnOnce(&T) -> R>(&'static self, f: F) -> R {
        let v = self.inner.get_or_init(self.init);
        f(v)
    }

    /// Like `with` but returns `Err(())` instead of initializing if
    /// the value hasn't been touched yet. Provided for parity with
    /// `std::thread::LocalKey::try_with`.
    pub fn try_with<R, F: FnOnce(&T) -> R>(&'static self, f: F) -> Result<R, ()> {
        match self.inner.get() {
            Some(v) => Ok(f(v)),
            None => Err(()),
        }
    }
}

/// `std::thread_local!`-shaped macro. Single-threaded variant: each
/// declared static becomes a process-wide lazy cell (see `LocalKey`).
///
/// Usage:
/// ```ignore
/// semos_std::thread_local! {
///     static FOO: u32 = 42;
///     pub static BAR: alloc::string::String = alloc::string::String::new();
/// }
/// ```
///
/// `with` and `try_with` mirror `std::thread::LocalKey` for source
/// compatibility — the rustc crates that import this expect the same
/// shape.
#[macro_export]
macro_rules! thread_local {
    () => {};
    ($(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty = $init:expr; $($rest:tt)*) => {
        $(#[$attr])*
        $vis static $name: $crate::thread::LocalKey<$ty> =
            $crate::thread::LocalKey::new(|| $init);
        $crate::thread_local! { $($rest)* }
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty = $init:expr) => {
        $(#[$attr])*
        $vis static $name: $crate::thread::LocalKey<$ty> =
            $crate::thread::LocalKey::new(|| $init);
    };
}

// ---------------------------------------------------------------------
// ScopedKey<T> + scoped_thread_local! macro — M27 R4 B2 (scoped-tls
// crate replacement).
//
// rustc_span uses the `scoped-tls` crate via the `scoped_thread_local!`
// macro for SESSION_GLOBALS — "set a value for the duration of a closure
// call, then unset." This shim provides the same shape. Single-threaded
// because SemOS Ring-3 is single-threaded; for multi-threading, replace
// the inner storage with a per-tid map.
// ---------------------------------------------------------------------

/// `scoped_tls::ScopedKey`-shaped: holds an `Option<*const T>` that's
/// set for the duration of a `set` call and unset afterwards. `with`
/// panics if no value is currently set; `is_set` lets callers check.
pub struct ScopedKey<T: 'static> {
    inner: core::cell::Cell<*const T>,
    _marker: core::marker::PhantomData<T>,
}

// Safety: SemOS Ring-3 is single-threaded; the Cell is only ever
// accessed from one thread. Sync is asserted so the static can live
// at program scope.
unsafe impl<T: 'static> Sync for ScopedKey<T> {}

impl<T: 'static> ScopedKey<T> {
    /// Build an empty key. Usually invoked by the
    /// `scoped_thread_local!` macro rather than directly.
    pub const fn new() -> Self {
        Self {
            inner: core::cell::Cell::new(core::ptr::null()),
            _marker: core::marker::PhantomData,
        }
    }

    /// Set the value for the duration of `f`'s execution. Panics if
    /// `set` is called recursively (i.e., already set).
    pub fn set<R, F: FnOnce() -> R>(&'static self, value: &T, f: F) -> R {
        let prev = self.inner.replace(value as *const T);
        // Defensive: scoped-tls panics on nested set; mirror that.
        // (rustc's SESSION_GLOBALS pattern only sets once per compile.)
        if !prev.is_null() {
            // Restore + panic.
            self.inner.set(prev);
            panic!("scoped_thread_local set recursively");
        }
        struct Guard<'a, T: 'static>(&'a ScopedKey<T>);
        impl<'a, T: 'static> Drop for Guard<'a, T> {
            fn drop(&mut self) {
                self.0.inner.set(core::ptr::null());
            }
        }
        let _guard = Guard(self);
        f()
    }

    /// Run `f` with the currently-set value. Panics if no value is set.
    pub fn with<R, F: FnOnce(&T) -> R>(&'static self, f: F) -> R {
        let p = self.inner.get();
        if p.is_null() {
            panic!("scoped_thread_local accessed without set");
        }
        // Safety: `set` guarantees `inner` is a valid `&T` while `f` is
        // running (the Drop guard unsets after f returns).
        unsafe { f(&*p) }
    }

    /// `true` iff a value is currently set on this key.
    pub fn is_set(&'static self) -> bool {
        !self.inner.get().is_null()
    }
}

/// `scoped_tls::scoped_thread_local!`-shaped macro. Single-threaded
/// variant: each declared static becomes a process-wide `ScopedKey`.
///
/// Usage:
/// ```ignore
/// semos_std::scoped_thread_local! {
///     pub static SESSION_GLOBALS: SessionGlobals;
/// }
///
/// SESSION_GLOBALS.set(&globals, || {
///     SESSION_GLOBALS.with(|g| use_it(g));
/// });
/// ```
#[macro_export]
macro_rules! scoped_thread_local {
    () => {};
    ($(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty; $($rest:tt)*) => {
        $(#[$attr])*
        $vis static $name: $crate::thread::ScopedKey<$ty> =
            $crate::thread::ScopedKey::new();
        $crate::scoped_thread_local! { $($rest)* }
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty) => {
        $(#[$attr])*
        $vis static $name: $crate::thread::ScopedKey<$ty> =
            $crate::thread::ScopedKey::new();
    };
}
