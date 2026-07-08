//! fb-demo.elf — M14-E Ring-3 framebuffer blit demo.
//!
//! Uses SYS_FB_META + SYS_FB_BLIT to draw a user-owned RGB buffer without
//! directly touching kernel framebuffer internals.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_WRITE: u64 = 0;
const SYS_EXIT: u64 = 2;
const SYS_FB_META: u64 = 128;
const SYS_FB_BLIT: u64 = 129;

const W: usize = 320;
const H: usize = 180;
static mut PIXELS: [u32; W * H] = [0; W * H];

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    let mut meta = [0u64; 8];
    let rc = unsafe { syscall2(SYS_FB_META, meta.as_mut_ptr() as u64, 64) };
    if rc == u64::MAX {
        write(b"fb-demo: framebuffer metadata unavailable\n");
        unsafe { sys_exit(1) }
    }

    let fb_w = meta[0] as usize;
    let fb_h = meta[1] as usize;
    let x = fb_w.saturating_sub(W) / 2;
    let y = fb_h.saturating_sub(H) / 2;

    unsafe {
        let base = core::ptr::addr_of_mut!(PIXELS) as *mut u32;
        for row in 0..H {
            for col in 0..W {
                let r = ((col * 255) / (W - 1)) as u32;
                let g = ((row * 255) / (H - 1)) as u32;
                let border = row < 4 || col < 4 || row + 4 >= H || col + 4 >= W;
                let b = if border { 255 } else { 64 };
                let rgb = (r << 16) | (g << 8) | b;
                core::ptr::write(base.add(row * W + col), rgb);
            }
        }
        let xy = (x as u64) | ((y as u64) << 32);
        let wh = (W as u64) | ((H as u64) << 32);
        let rc = syscall4(SYS_FB_BLIT, xy, wh, base as u64, (W * H) as u64);
        if rc == u64::MAX {
            write(b"fb-demo: SYS_FB_BLIT failed\n");
            sys_exit(1)
        }
    }

    write(b"fb-demo: drew 320x180 user RGB buffer via SYS_FB_BLIT\n");
    unsafe { sys_exit(0) }
}

fn write(bytes: &[u8]) {
    unsafe { let _ = syscall2(SYS_WRITE, bytes.as_ptr() as u64, bytes.len() as u64); }
}

#[inline(always)]
unsafe fn syscall2(num: u64, a: u64, b: u64) -> u64 {
    syscall4(num, a, b, 0, 0)
}

#[inline(always)]
unsafe fn syscall4(num: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
    let ret: u64;
    asm!(
        "syscall",
        in("rax") num,
        in("rdi") a,
        in("rsi") b,
        in("rdx") c,
        in("r10") d,
        lateout("rax") ret,
        out("rcx") _,
        out("r11") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    asm!("syscall", in("rax") SYS_EXIT, in("rdi") code, options(nostack));
    loop {}
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    unsafe { sys_exit(1) }
}
