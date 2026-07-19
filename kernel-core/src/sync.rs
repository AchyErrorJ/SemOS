//! Kernel synchronization primitives.
//!
//! `Mutex` is the answer to the 2026-07-17 review's P1 finding: the syscall
//! layer's shared singletons (semantic registry, LLM redactor/summarizer,
//! vouch table, scratch buffers) were `static mut` borrowed as `&'static mut`
//! under the assumption that "syscalls are serialized". That assumption is
//! false: several handlers enable interrupts mid-flight (`llm_ask`,
//! `run_agent_tui`, `run_editor`, `run_usbenum`), and a preempted handler can
//! be interleaved with another task's syscall touching the same global —
//! a data race through `&'static mut` (UB) and a correctness bug
//! (interleaved LLM-context output).
//!
//! ## Why yield-on-contention instead of a bare spinlock
//!
//! Syscall handlers run with interrupts OFF (the `syscall` entry masks IF),
//! so a task that finds the mutex contended cannot simply spin: the timer
//! would never fire, the holder would never be rescheduled, and the spin
//! would deadlock the machine. The slow path therefore calls
//! `platform::schedule()` — the same voluntary-yield pattern `SYS_SLEEP`
//! already uses — so the holder gets the CPU back. On the host test runner
//! there is no platform; `NullPlatform::schedule()` is a no-op and the OS
//! preempts the spinning thread instead, which is equally live.
//!
//! ## Rules for holders
//!
//! - Never call `platform::schedule()` (or anything that can block, e.g.
//!   `SYS_SLEEP`, pipe waits) while holding a guard — other lockers would
//!   keep yielding until you return, which is latency, not deadlock, but
//!   don't do it.
//! - Never acquire the same mutex twice on one call path (nested
//!   `dispatch()` from agent/editor code must run guard-free) — the mutex
//!   is non-recursive and a self-acquisition yields forever.
//! - ISRs must never take these mutexes (a guard's slow path yields from
//!   interrupt context). The timer/keyboard/serial ISRs don't touch any of
//!   the protected globals — keep it that way.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Yield-on-contention mutual exclusion for kernel singletons. See the
/// module docs for the design contract.
pub struct Mutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

/// RAII guard; releases the mutex on drop. Derefs to `T` so existing
/// `registry.get(...)`-style call sites read unchanged.
pub struct MutexGuard<'a, T> {
    lock: &'a Mutex<T>,
}

// The mutex provides mutual exclusion over `data`, so sharing `&Mutex<T>`
// across tasks is sound for any `T` (the same justification as
// `spin::Mutex<T>: Sync` where the payload need only be `Send`; here tasks
// are the kernel's green threads, and `T` never crosses to host threads in
// kernel use — host unit tests are the only multi-threaded case and they
// only touch `T` through the guard).
unsafe impl<T> Sync for Mutex<T> {}
unsafe impl<T> Send for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the mutex, yielding the timeslice while contended.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return MutexGuard { lock: self };
            }
            // Contended. IRQs are off in syscall context, so a bare spin
            // would never let the holder run — yield instead.
            core::hint::spin_loop();
            crate::platform::schedule();
        }
    }

    /// Acquire without yielding. For call sites that provably cannot yield
    /// (none today — kept for completeness).
    #[allow(dead_code)]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| MutexGuard { lock: self })
    }
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_unlock_cycle() {
        static M: Mutex<u64> = Mutex::new(0);
        {
            let mut g = M.lock();
            *g += 41;
        }
        let g = M.lock();
        assert_eq!(*g, 41);
    }

    #[test]
    fn test_try_lock_contended() {
        static M: Mutex<u64> = Mutex::new(0);
        let g1 = M.lock();
        assert!(M.try_lock().is_none());
        drop(g1);
        assert!(M.try_lock().is_some());
    }
}
