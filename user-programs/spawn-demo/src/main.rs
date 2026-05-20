//! spawn-demo.elf — Phase 14 M25 `std::process::Command` acceptance test.
//!
//! A Ring-3 program that uses `semos_std::process::Command` to spawn other
//! `/bin` programs as child processes and block on their exit codes via
//! SYS_SPAWN + SYS_WAIT. This is the first time a *Ring-3 parent* (not the
//! kernel) drives spawn+wait, exercising the SYS_WAIT slot-join path and
//! the per-process address-space isolation fix (a child's PML4 is copied
//! from the clean kernel page tables, not the caller's).
//!
//! Built at opt-level=0 (task #54 — optimization miscompiles the syscall
//! path). Avoids number formatting; signals via exit codes.
//!
//! Checks:
//!   1. Command::new("/bin/hello-std").status() succeeds (child exits 0).
//!   2. Same with an arg — exercises the argv-blob path; child still 0.
//!   3. Command::new("/bin/thread-demo").status() == 0x2700 — proves a
//!      non-zero child exit code propagates back through SYS_WAIT, and that
//!      a child which itself spawns Ring-3 threads runs to completion.
//!
//! Exit codes (read by the kernel in DEMO 32):
//!   0     — full pass
//!   0x51  — hello-std spawn/wait failed or non-zero status
//!   0x52  — hello-std-with-arg spawn/wait failed
//!   0x53  — thread-demo exit code != 0x2700

#![no_std]
#![no_main]

use semos_std::{main, println};
use semos_std::process::{self, Command};

main!(fn main() {
    println!("spawn-demo: started");

    // 1: no-arg spawn + wait, expect success (exit 0).
    match Command::new("/bin/hello-std").status() {
        Ok(st) if st.success() => {
            println!("spawn-demo: PASS hello-std exited 0");
        }
        _ => {
            println!("spawn-demo: FAIL hello-std status");
            process::exit(0x51);
        }
    }

    // 2: spawn with an argument — exercises the argv-blob path. hello-std
    //    ignores argv, so we just require a successful exit.
    match Command::new("/bin/hello-std").arg("from-spawn-demo").status() {
        Ok(st) if st.success() => {
            println!("spawn-demo: PASS hello-std (with arg) exited 0");
        }
        _ => {
            println!("spawn-demo: FAIL hello-std with-arg status");
            process::exit(0x52);
        }
    }

    // 3: spawn a child with a known non-zero exit code (thread-demo exits
    //    0x2700) and confirm it propagates back through SYS_WAIT.
    match Command::new("/bin/thread-demo").status() {
        Ok(st) if st.code() == Some(0x2700) => {
            println!("spawn-demo: PASS thread-demo exit code 0x2700 propagated");
        }
        _ => {
            println!("spawn-demo: FAIL thread-demo exit code");
            process::exit(0x53);
        }
    }

    println!("spawn-demo: ALL CHECKS PASSED");
});
