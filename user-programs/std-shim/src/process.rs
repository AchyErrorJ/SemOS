//! `std::process`-shaped: exit, abort.
//!
//! `Command` / `Child` / `wait_with_output` land in M25 Tier 2 once
//! the kernel's SYS_WAIT path is solid for user-mode parents (today
//! it works through scheduler::task_state polling, not the
//! ProcessState path — see SYS_WAIT note in syscall/mod.rs).

use crate::arch::{SYS_EXIT, syscall1};

/// Terminate the current process with the given exit code. The kernel
/// transitions the task to Exited and yields to the scheduler.
pub fn exit(code: i32) -> ! {
    unsafe {
        let _ = syscall1(SYS_EXIT, code as u64);
    }
    // SYS_EXIT shouldn't return — but if it does, halt defensively.
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Abnormal termination — wraps `exit(101)` to match std's convention.
/// Used by the panic handler.
pub fn abort() -> ! {
    exit(101)
}
