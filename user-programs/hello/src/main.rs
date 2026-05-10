//! hello.elf — first real Rust user program for Semantic OS.
//!
//! Calls SYS_WRITE("Hello from real Rust ELF!\n") then SYS_EXIT(0).
//! Replaces the hand-assembled hello.elf in kernel-core/src/process/elf.rs.
//!
//! Built with: cargo build --release  (from this directory)
//! Output: target/x86_64-unknown-none/release/hello

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_WRITE: u64 = 0;
const SYS_EXIT:  u64 = 2;

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    static MSG: &[u8] = b"Hello from real Rust ELF!\n";
    unsafe {
        sys_write(MSG.as_ptr(), MSG.len() as u64);
        sys_exit(0)
    }
}

#[inline(always)]
unsafe fn sys_write(buf: *const u8, len: u64) -> u64 {
    let ret: u64;
    asm!(
        "syscall",
        in("rax") SYS_WRITE,
        in("rdi") buf as u64,
        in("rsi") len,
        lateout("rax") ret,
        out("rcx") _,
        out("r11") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    // SYS_EXIT marks the task as Exited but SYSRET still returns to user
    // mode. We must keep the user CPU in a benign instruction stream until
    // the next timer tick context-switches us out for good. A `loop {}`
    // after the syscall does this — without it, the compiler emits a `ud2`
    // and the kernel logs a noisy "INVALID OPCODE" for the dying task.
    asm!(
        "syscall",
        in("rax") SYS_EXIT,
        in("rdi") code,
        options(nostack),
    );
    loop {}
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    unsafe { sys_exit(1) }
}
