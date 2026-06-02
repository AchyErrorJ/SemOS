//! `std::sync`-shaped primitives over the kernel futex (M25 #52).
//!
//! Mutex and Once lower to SYS_FUTEX_WAIT / SYS_FUTEX_WAKE on a u32
//! state word — the same shape Linux std uses. Single-CPU + futex
//! makes the fast path a plain atomic CAS with no syscall; only
//! contention (or the wait side of Once) enters the kernel.
//!
//! Mutex / Once / Condvar / RwLock are all here; `mpsc` lives in its
//! own module on top of Mutex + Condvar.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};
use crate::arch::{SYS_FUTEX_WAIT, SYS_FUTEX_WAKE, syscall2};

// Re-export Arc from the alloc crate so `semos_std::sync::Arc` works
// like `std::sync::Arc` (alloc owns the Arc impl; std just re-exports).
pub use core_alloc::sync::Arc;

// Re-export `mpsc` here under `sync::` to mirror std's path
// (`std::sync::mpsc::*`). The actual impl lives at `crate::mpsc`; this
// is just a path-discoverability fix. (Phase 4 G1 flagged that
// `semos_std::sync::mpsc` wasn't finding the existing module.)
pub use crate::mpsc;

// Mutex state values.
const UNLOCKED: u32 = 0;
const LOCKED: u32 = 1;
const LOCKED_CONTENDED: u32 = 2;

#[inline]
fn futex_wait(word: &AtomicU32, expected: u32) {
    unsafe {
        let _ = syscall2(
            SYS_FUTEX_WAIT,
            word as *const _ as u64,
            expected as u64,
        );
    }
}

#[inline]
fn futex_wake_one(word: &AtomicU32) {
    unsafe {
        let _ = syscall2(SYS_FUTEX_WAKE, word as *const _ as u64, 1);
    }
}

#[inline]
fn futex_wake_all(word: &AtomicU32) {
    unsafe {
        let _ = syscall2(SYS_FUTEX_WAKE, word as *const _ as u64, u64::MAX);
    }
}

// ---------------------------------------------------------------------
// Mutex
// ---------------------------------------------------------------------

/// A futex-backed mutual-exclusion lock. Same API shape as
/// `std::sync::Mutex` (lock returns a guard; no poisoning).
pub struct Mutex<T> {
    state: AtomicU32,
    data: UnsafeCell<T>,
}

// SAFETY: the futex serializes access; T need only be Send.
unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> core::fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Mutex { .. }")
    }
}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicU32::new(UNLOCKED),
            data: UnsafeCell::new(value),
        }
    }

    /// Lock, blocking via futex on contention. Returns a guard.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        // Fast path: UNLOCKED → LOCKED.
        if self
            .state
            .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return MutexGuard { mutex: self };
        }
        self.lock_contended();
        MutexGuard { mutex: self }
    }

    #[cold]
    fn lock_contended(&self) {
        // Mark contended and park until woken. Classic 3-state futex
        // mutex (Drepper's "Futexes Are Tricky").
        loop {
            // Try to grab it directly first.
            if self
                .state
                .compare_exchange(UNLOCKED, LOCKED_CONTENDED, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            // Ensure the word reads CONTENDED so the unlocker knows to wake us.
            let prev = self.state.swap(LOCKED_CONTENDED, Ordering::Acquire);
            if prev == UNLOCKED {
                return;
            }
            // Wait while it's still contended.
            futex_wait(&self.state, LOCKED_CONTENDED);
        }
    }

    /// Try to lock without blocking.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self
            .state
            .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(MutexGuard { mutex: self })
        } else {
            None
        }
    }

    unsafe fn unlock(&self) {
        // If there were waiters (state was CONTENDED), wake one.
        if self.state.swap(UNLOCKED, Ordering::Release) == LOCKED_CONTENDED {
            futex_wake_one(&self.state);
        }
    }
}

/// RAII guard; unlocks on Drop.
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: we hold the lock.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: we hold the lock exclusively.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        // SAFETY: guard existence proves the lock is held.
        unsafe { self.mutex.unlock() }
    }
}

// ---------------------------------------------------------------------
// Once
// ---------------------------------------------------------------------

const ONCE_INCOMPLETE: u32 = 0;
const ONCE_RUNNING: u32 = 1;
const ONCE_COMPLETE: u32 = 2;

/// One-time initialization, futex-backed. `call_once` runs the closure
/// exactly once across all callers; concurrent callers block until the
/// first completes. Mirrors `std::sync::Once`.
pub struct Once {
    state: AtomicU32,
}

impl Once {
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(ONCE_INCOMPLETE),
        }
    }

    pub fn is_completed(&self) -> bool {
        self.state.load(Ordering::Acquire) == ONCE_COMPLETE
    }

    pub fn call_once<F: FnOnce()>(&self, f: F) {
        // Fast path: already done.
        if self.state.load(Ordering::Acquire) == ONCE_COMPLETE {
            return;
        }
        loop {
            match self.state.compare_exchange(
                ONCE_INCOMPLETE,
                ONCE_RUNNING,
                Ordering::Acquire,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // We won the race — run the init, then publish + wake.
                    f();
                    self.state.store(ONCE_COMPLETE, Ordering::Release);
                    futex_wake_all(&self.state);
                    return;
                }
                Err(ONCE_COMPLETE) => return,
                Err(_) => {
                    // Another caller is running it; park until complete.
                    futex_wait(&self.state, ONCE_RUNNING);
                }
            }
        }
    }
}

impl Default for Once {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------
// OnceLock<T> — futex-backed lazy-initialized cell holding a T
// ---------------------------------------------------------------------
//
// M27 D.2-followup discovered we needed a no_std OnceLock for Cranelift.
// We wrote a local shim there (cranelift-codegen/src/isa/x64/abi.rs) and
// the recon agents confirmed 8+ rustc_* crates also want it. Lifted
// into semos-std as the canonical shim — futex-aware (so it's safe for
// concurrent first-call across multiple threads) rather than the
// single-threaded AtomicBool+UnsafeCell version Cranelift used.
//
// API mirrors `std::sync::OnceLock`:
// - `new()` const-fn empty cell
// - `get()` → Option<&T>
// - `get_or_init(f)` → &T, runs f exactly once across callers
// - `set(value)` → Result<(), T> (Err if already initialized)

const LOCK_EMPTY: u32 = 0;
const LOCK_INITIALIZING: u32 = 1;
const LOCK_INITIALIZED: u32 = 2;

/// `std::sync::OnceLock`-shaped lazy-initialized cell holding a `T`.
/// Initialization runs exactly once, with concurrent callers parking
/// on the futex until the first finishes.
pub struct OnceLock<T> {
    state: AtomicU32,
    value: UnsafeCell<core::mem::MaybeUninit<T>>,
}

// Safety: the state-machine + futex serialize all access to `value`.
// Readers only ever see `value` after it's been written and state is
// LOCK_INITIALIZED (with Acquire ordering).
unsafe impl<T: Send + Sync> Sync for OnceLock<T> {}
unsafe impl<T: Send> Send for OnceLock<T> {}

impl<T> OnceLock<T> {
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(LOCK_EMPTY),
            value: UnsafeCell::new(core::mem::MaybeUninit::uninit()),
        }
    }

    /// Returns `Some(&T)` if already initialized, else `None`.
    pub fn get(&self) -> Option<&T> {
        if self.state.load(Ordering::Acquire) == LOCK_INITIALIZED {
            Some(unsafe { (*self.value.get()).assume_init_ref() })
        } else {
            None
        }
    }

    /// Returns a reference to the value, initializing it via `f` if
    /// not yet initialized. `f` runs at most once across all callers.
    pub fn get_or_init<F: FnOnce() -> T>(&self, f: F) -> &T {
        // Fast path: already initialized.
        if self.state.load(Ordering::Acquire) == LOCK_INITIALIZED {
            return unsafe { (*self.value.get()).assume_init_ref() };
        }
        loop {
            match self.state.compare_exchange(
                LOCK_EMPTY,
                LOCK_INITIALIZING,
                Ordering::Acquire,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // We won — run init, publish, wake waiters.
                    let v = f();
                    unsafe { (*self.value.get()).write(v); }
                    self.state.store(LOCK_INITIALIZED, Ordering::Release);
                    futex_wake_all(&self.state);
                    return unsafe { (*self.value.get()).assume_init_ref() };
                }
                Err(LOCK_INITIALIZED) => {
                    return unsafe { (*self.value.get()).assume_init_ref() };
                }
                Err(_) => {
                    // Another caller is initializing; park until done.
                    futex_wait(&self.state, LOCK_INITIALIZING);
                }
            }
        }
    }

    /// Sets the value if uninitialized. Returns the input value as
    /// `Err(value)` if already initialized.
    pub fn set(&self, value: T) -> Result<(), T> {
        if self.state.compare_exchange(
            LOCK_EMPTY,
            LOCK_INITIALIZING,
            Ordering::Acquire,
            Ordering::Acquire,
        ).is_ok() {
            unsafe { (*self.value.get()).write(value); }
            self.state.store(LOCK_INITIALIZED, Ordering::Release);
            futex_wake_all(&self.state);
            Ok(())
        } else {
            // Either INITIALIZING or INITIALIZED. Wait if initializing
            // so the caller doesn't race; then report the slot full.
            while self.state.load(Ordering::Acquire) == LOCK_INITIALIZING {
                futex_wait(&self.state, LOCK_INITIALIZING);
            }
            Err(value)
        }
    }
}

impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for OnceLock<T> {
    fn drop(&mut self) {
        if self.state.load(Ordering::Relaxed) == LOCK_INITIALIZED {
            unsafe {
                (*self.value.get()).assume_init_drop();
            }
        }
    }
}

// ---------------------------------------------------------------------
// LazyLock<T> — `std::sync::LazyLock` over OnceLock + fn() -> T
// ---------------------------------------------------------------------
//
// rustc and several externals expect `LazyLock<T, fn() -> T>` (or a
// closure init). v1 limits the init to a `fn` pointer (not a closure)
// since that's what makes a static `LazyLock::new(|| {...})` const-
// constructible without dropping into a generic closure capture.
// Multiple rustc_* crates rely on this exact shape (C3 flagged it as
// the LazyLock gap).

/// `std::sync::LazyLock`-shaped lazy-initialized cell. The init `fn`
/// runs once on first `Deref` (or `force`), and the value persists.
pub struct LazyLock<T> {
    cell: OnceLock<T>,
    init: fn() -> T,
}

unsafe impl<T: Send + Sync> Sync for LazyLock<T> {}

impl<T> LazyLock<T> {
    /// Build a LazyLock backed by `init`. Const so it can be a static.
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            cell: OnceLock::new(),
            init,
        }
    }

    /// Force initialization and return a reference. Mirrors std's
    /// `LazyLock::force`.
    pub fn force(this: &Self) -> &T {
        this.cell.get_or_init(this.init)
    }
}

impl<T> Deref for LazyLock<T> {
    type Target = T;
    fn deref(&self) -> &T {
        Self::force(self)
    }
}

// ---------------------------------------------------------------------
// Condvar — futex-backed condition variable (seq-number style)
// ---------------------------------------------------------------------
//
// A condvar's job is "park me until someone says state has changed."
// The futex pattern is a sequence counter that increments on every
// notify_*; waiters read the seq, drop the mutex, futex-wait on the
// OLD seq value, then re-acquire the mutex. Any notify between the
// read and the wait advances the counter so the wait returns
// immediately (no lost wakeup).

/// `std::sync::Condvar`-shaped condition variable. Pair with a Mutex.
pub struct Condvar {
    seq: AtomicU32,
}
impl core::fmt::Debug for Condvar {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Condvar { .. }")
    }
}

impl Condvar {
    pub const fn new() -> Self {
        Self { seq: AtomicU32::new(0) }
    }

    /// Atomically drop `guard`, park until notified, then re-lock and
    /// return a fresh guard. Spurious wakes are possible — always re-
    /// check the condition in a `while` loop.
    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        let mutex = guard.mutex;
        let s = self.seq.load(Ordering::Acquire);
        drop(guard); // releases the mutex
        futex_wait(&self.seq, s);
        mutex.lock()
    }

    pub fn notify_one(&self) {
        self.seq.fetch_add(1, Ordering::Release);
        futex_wake_one(&self.seq);
    }

    pub fn notify_all(&self) {
        self.seq.fetch_add(1, Ordering::Release);
        futex_wake_all(&self.seq);
    }
}

impl Default for Condvar {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------
// RwLock
// ---------------------------------------------------------------------
//
// Single u32 state:
//   bit 31:     write lock held
//   bits 30:0:  reader count (only meaningful when bit 31 is 0)
//
// Read lock: CAS to increment when bit 31 is clear; otherwise futex-wait.
// Write lock: CAS 0 -> WRITE_LOCKED; otherwise futex-wait.
// Unlocking either path wakes all (waiters may be readers or a writer;
// we don't track which, so wake everyone and let them re-CAS).
//
// v1 — no writer preference / fairness; under heavy reader load a
// writer can starve. Fine for the typical kernel-app workload here.

const WRITE_LOCKED: u32 = 1u32 << 31;

pub struct RwLock<T> {
    state: AtomicU32,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    pub const fn new(v: T) -> Self {
        Self { state: AtomicU32::new(0), data: UnsafeCell::new(v) }
    }

    /// Acquire a shared read lock. Blocks via futex if a writer holds it.
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        loop {
            let s = self.state.load(Ordering::Acquire);
            if s & WRITE_LOCKED == 0 {
                if self
                    .state
                    .compare_exchange(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return RwLockReadGuard { lock: self };
                }
                // CAS failed — another acquire raced; retry the load.
                continue;
            }
            futex_wait(&self.state, s);
        }
    }

    /// Acquire the exclusive write lock. Blocks via futex if there are
    /// any readers or a writer.
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        loop {
            let s = self.state.load(Ordering::Acquire);
            if s == 0 {
                if self
                    .state
                    .compare_exchange(0, WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return RwLockWriteGuard { lock: self };
                }
                continue;
            }
            futex_wait(&self.state, s);
        }
    }

    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        let s = self.state.load(Ordering::Acquire);
        if s & WRITE_LOCKED != 0 {
            return None;
        }
        self.state
            .compare_exchange(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| RwLockReadGuard { lock: self })
    }

    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        self.state
            .compare_exchange(0, WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| RwLockWriteGuard { lock: self })
    }
}

pub struct RwLockReadGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockReadGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: holding a read lock means no writer is active.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        // Last reader out wakes anyone (likely a writer) blocked on us.
        if self.lock.state.fetch_sub(1, Ordering::Release) == 1 {
            futex_wake_all(&self.lock.state);
        }
    }
}

pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}
impl<T> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        // Release the write bit and wake all waiters.
        self.lock.state.store(0, Ordering::Release);
        futex_wake_all(&self.lock.state);
    }
}
