#![no_std]
#![no_main]
use core::arch::asm;
use core::panic::PanicInfo;
const SYS_WRITE:u64=0; const SYS_EXIT:u64=2;
#[no_mangle]
#[link_section=".text._start"]
pub extern "C" fn _start() -> ! { static MSG:&[u8]=b"fb-demo: M14 framebuffer syscall demo placeholder; use `fbinfo` for display diagnostics.\n"; unsafe{ sys_write(MSG.as_ptr(), MSG.len() as u64); sys_exit(0) }}
#[inline(always)] unsafe fn sys_write(buf:*const u8,len:u64)->u64{let ret:u64; asm!("syscall", in("rax") SYS_WRITE, in("rdi") buf as u64, in("rsi") len, lateout("rax") ret, out("rcx") _, out("r11") _, options(nostack)); ret}
#[inline(always)] unsafe fn sys_exit(code:u64)->!{asm!("syscall", in("rax") SYS_EXIT, in("rdi") code, options(nostack)); loop{}}
#[panic_handler] fn panic(_: &PanicInfo)->!{unsafe{sys_exit(1)}}
