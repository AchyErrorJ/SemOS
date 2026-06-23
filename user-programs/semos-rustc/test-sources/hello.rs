// DEMO 80 source: a self-hosting "hello world" compiled by the SemOS-resident
// `semos-rustc` to a native ELF, loaded and run by the kernel on bare metal.
//
// `#![no_std]` — the SemOS-target rustc has no std crate.
// `#![no_main]` — SemOS user programs supply their own `_start`.
// It writes a greeting to stdout (fd 1) and exits cleanly via the SemOS
// syscall ABI (rax = number, args in rdi/rsi/rdx, `syscall` instruction).
#![no_std]
#![no_main]

use core::arch::asm;

const SYS_WRITE: u64 = 0;
const SYS_EXIT: u64 = 2;

#[inline(always)]
unsafe fn syscall3(num: u64, a: u64, b: u64, c: u64) -> u64 {
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
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let msg = b"Hello, world from bare-metal semos-rustc!\n";
    unsafe {
        syscall3(SYS_WRITE, 1, msg.as_ptr() as u64, msg.len() as u64);
        syscall3(SYS_EXIT, 0, 0, 0);
    }
    loop {}
}
