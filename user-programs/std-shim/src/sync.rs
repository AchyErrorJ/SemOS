//! `std::sync`-shaped primitives over the kernel futex (M25 #52).
//!
//! Mutex and Once lower to SYS_FUTEX_WAIT / SYS_FUTEX_WAKE on a u32
//! state word — the same shape Linux std uses. Single-CPU + futex
//! makes the fast path a plain atomic CAS with no syscall; only
//! contention (or the wait side of Once) enters the kernel.
//!
//! Condvar and RwLock aren't here yet — Mutex + Once cover what a
//! first threaded program (and `OnceLock`-style lazy init) need; the
//! rest follow the same futex pattern when a caller requires them.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};
use crate::arch::{SYS_FUTEX_WAIT, SYS_FUTEX_WAKE, syscall2};

// Re-export Arc from the alloc crate so `semos_std::sync::Arc` works
// like `std::sync::Arc` (alloc owns the Arc impl; std just re-exports).
pub use core_alloc::sync::Arc;

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
