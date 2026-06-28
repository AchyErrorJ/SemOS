// DEMO 80 source: a self-hosting "hello world" compiled by the SemOS-resident
// `semos-rustc` to a native ELF, loaded and run by the kernel on bare metal.
//
// `#![no_std]` — the SemOS-target rustc has no std crate.
// `#![no_main]` — SemOS user programs supply their own `_start`.
//
// It calls two `extern "C"` syscall stubs (`sys_write`, `sys_exit`). cg_clif
// on SemOS has no assembler, so inline `asm!` can't be used; instead the
// semos-rustc linker injects tiny pre-assembled stubs (`mov eax,N; syscall;
// ret`) for these well-known names — a minimal crt. The SysV argument
// registers (rdi/rsi/rdx) already match the SemOS syscall ABI for <=3 args.
#![no_std]
#![no_main]

extern "C" {
    fn sys_write(fd: u64, buf: *const u8, len: u64) -> i64;
    fn sys_exit(code: u64) -> !;
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let msg = b"Hello, world from bare-metal semos-rustc!\n";
    unsafe {
        sys_write(1, msg.as_ptr(), msg.len() as u64);
        sys_exit(0);
    }
    loop {}
}
