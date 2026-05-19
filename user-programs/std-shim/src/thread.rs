//! Threading + sleep — minimal subset of `std::thread`.
//!
//! Phase 14 Tier 3 prereqs landed all the primitives; this module
//! is the std-shim wrapper. Today it covers raw sleep + thread spawn
//! via the SYS_THREAD_SPAWN syscall. JoinHandle / scope handling and
//! `std::sync::Mutex` / `Condvar` (futex-backed) follow in M25 Tier 2.

use crate::arch::{SYS_SLEEP, SYS_THREAD_SPAWN, SYS_THREAD_JOIN, syscall1, syscall2};

/// Block the current task for `ticks` scheduler timer ticks.
///
/// Real `Duration` → tick conversion uses `kernel-core::scheduler::
/// SCHEDULER_TICK_HZ` (62 Hz on QEMU; varies on hardware). The std
/// shim's `Duration::from_*` constructor would convert internally;
/// for the M25-Tier-1 hello path, callers can use tick counts directly.
pub fn sleep_ticks(ticks: u64) {
    unsafe { let _ = syscall1(SYS_SLEEP, ticks); }
}

/// Spawn a new thread sharing the current process's address space.
/// Returns a tid (scheduler slot index) on success, `u64::MAX` on error.
///
/// The new thread starts at `entry`, with `arg` passed in rdi (SysV
/// AMD64 first-arg). The thread should call [`super::process::exit`]
/// to terminate; falling off the end isn't supported yet.
///
/// This is the raw u64-tid version. A `JoinHandle<T>`-shaped wrapper
/// over RAII semantics is M25 Tier 2.
pub fn spawn_raw(entry: extern "C" fn(u64) -> !, arg: u64) -> u64 {
    unsafe {
        syscall2(SYS_THREAD_SPAWN, entry as u64, arg)
    }
}

/// Wait for thread `tid` to exit; returns its exit code.
pub fn join_raw(tid: u64) -> u64 {
    unsafe { syscall1(SYS_THREAD_JOIN, tid) }
}
