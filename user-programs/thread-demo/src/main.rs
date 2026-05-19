//! thread-demo.elf — Ring 3 validation for SYS_THREAD_SPAWN / FUTEX / JOIN.
//!
//! This is the Phase 14 Tier 3 (#45 Ring-3 branch) acceptance test: a
//! user-mode program spawns a sibling thread sharing its address space,
//! synchronises on a u32 futex word, then joins to harvest the
//! sibling's exit code. The kernel runs this as DEMO 28 by spawning it
//! via SYS_SPAWN and reading the parent's exit status with SYS_WAIT.
//!
//! Exit codes (read by the kernel via SYS_WAIT in DEMO 28):
//!   0x2700 — full pass (thread spawned, futex woke, join returned 0x55)
//!   0x2701 — thread spawn failed
//!   0x2702 — value-mismatch FUTEX_WAIT didn't return 1
//!   0x2703 — FUTEX_WAKE didn't wake exactly one waiter
//!   0x2704 — join returned wrong code
//!   0x27ff — generic fail (catch-all)

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering};

const SYS_WRITE:        u64 = 0;
const SYS_EXIT:         u64 = 2;
const SYS_SLEEP:        u64 = 5;
const SYS_FUTEX_WAIT:   u64 = 90;
const SYS_FUTEX_WAKE:   u64 = 91;
const SYS_THREAD_SPAWN: u64 = 92;
const SYS_THREAD_JOIN:  u64 = 93;

// Shared futex word in the process's data segment. The new thread
// inherits the address space so it sees the same `FUTEX` static at the
// same virtual address.
#[repr(align(4))]
struct FutexWord(AtomicU32);
static FUTEX: FutexWord = FutexWord(AtomicU32::new(0));

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    unsafe {
        sys_write(b"thread-demo: entered\n".as_ptr(), 21);

        // Step 1: FUTEX_WAIT mismatch path returns 1 without blocking.
        FUTEX.0.store(7, Ordering::SeqCst);
        let r = sys(SYS_FUTEX_WAIT, &FUTEX.0 as *const _ as u64, 0, 0);
        if r != 1 {
            sys_write(b"thread-demo: mismatch path FAIL\n".as_ptr(), 33);
            sys_exit(0x2702);
        }

        // Step 2: Reset, spawn the sibling thread, give it time to block.
        FUTEX.0.store(0, Ordering::SeqCst);
        let tid = sys(SYS_THREAD_SPAWN, thread_entry as u64, 0x55, 0);
        if tid == u64::MAX {
            sys_write(b"thread-demo: spawn FAIL\n".as_ptr(), 25);
            sys_exit(0x2701);
        }
        sys(SYS_SLEEP, 10, 0, 0); // ~100 ms, plenty for sibling to block

        // Step 3: Wake the sibling.
        let woken = sys(SYS_FUTEX_WAKE, &FUTEX.0 as *const _ as u64, 1, 0);
        if woken != 1 {
            sys_write(b"thread-demo: wake FAIL\n".as_ptr(), 24);
            sys_exit(0x2703);
        }

        // Step 4: Join and check the sibling's exit code (which is `arg`).
        let code = sys(SYS_THREAD_JOIN, tid, 0, 0);
        if code != 0x55 {
            sys_write(b"thread-demo: join wrong code\n".as_ptr(), 30);
            sys_exit(0x2704);
        }

        sys_write(b"thread-demo: PASS\n".as_ptr(), 18);
        sys_exit(0x2700);
    }
}

/// Sibling thread entry. The kernel hands us `arg` in rdi; we futex-wait
/// then exit with `arg` as our code so the parent's JOIN sees it back.
extern "C" fn thread_entry(arg: u64) -> ! {
    unsafe {
        sys_write(b"thread-demo: thread started, calling FUTEX_WAIT\n".as_ptr(), 48);
        let r = sys(SYS_FUTEX_WAIT, &FUTEX.0 as *const _ as u64, 0, 0);
        if r != 0 {
            // mismatch race — exit with a distinguishable code so the
            // parent's join sees something other than the expected 0x55.
            sys_exit(0xDEAD);
        }
        sys_write(b"thread-demo: thread woke, exiting\n".as_ptr(), 35);
        sys_exit(arg);
    }
}

#[inline(always)]
unsafe fn sys_write(buf: *const u8, len: u64) {
    asm!(
        "syscall",
        in("rax") SYS_WRITE,
        in("rdi") buf as u64,
        in("rsi") len,
        lateout("rax") _,
        out("rcx") _,
        out("r11") _,
        options(nostack),
    );
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    asm!(
        "syscall",
        in("rax") SYS_EXIT,
        in("rdi") code,
        options(nostack),
    );
    loop {}
}

#[inline(always)]
unsafe fn sys(num: u64, a: u64, b: u64, c: u64) -> u64 {
    let ret: u64;
    asm!(
        "syscall",
        in("rax") num,
        in("rdi") a,
        in("rsi") b,
        in("rdx") c,
        lateout("rax") ret,
        out("rcx") _,
        out("r11") _,
        options(nostack),
    );
    ret
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    unsafe { sys_exit(0x27ff) }
}
