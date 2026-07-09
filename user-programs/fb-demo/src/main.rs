//! fb-demo.elf — M14-H vsync-paced Ring-3 framebuffer animation.
//!
//! Queries framebuffer metadata (SYS_FB_META), then animates a centered color
//! field for a fixed number of frames. Each frame waits for a display frame
//! boundary (SYS_FB_WAIT_VBLANK) before presenting a user-owned RGB buffer
//! (SYS_FB_BLIT) — tear-reduced ~60 FPS without any kernel-side mode writes.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_WRITE: u64 = 0;
const SYS_EXIT: u64 = 2;
const SYS_FB_META: u64 = 128;
const SYS_FB_BLIT: u64 = 129;
const SYS_FB_WAIT_VBLANK: u64 = 131;

const W: usize = 640;
const H: usize = 360;
const FRAMES: u32 = 240;
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
    let xy = (x as u64) | ((y as u64) << 32);
    let wh = (W as u64) | ((H as u64) << 32);

    let mut paced: u32 = 0;
    let base = core::ptr::addr_of_mut!(PIXELS) as *mut u32;

    for frame in 0..FRAMES {
        let p = frame.wrapping_mul(3);
        unsafe {
            for row in 0..H {
                let gy = ((row as u32).wrapping_add(p / 2) & 0xFF) as u32;
                for col in 0..W {
                    let r = ((col as u32).wrapping_add(p) & 0xFF) as u32;
                    let b = (((col + row) as u32).wrapping_add(p) & 0xFF) as u32;
                    let rgb = (r << 16) | (gy << 8) | b;
                    core::ptr::write(base.add(row * W + col), rgb);
                }
            }
            // Pace to a frame boundary; count how often the source is available.
            if syscall1(SYS_FB_WAIT_VBLANK, 0) == 0 {
                paced += 1;
            }
            let rc = syscall4(SYS_FB_BLIT, xy, wh, base as u64, (W * H) as u64);
            if rc == u64::MAX {
                write(b"fb-demo: SYS_FB_BLIT failed\n");
                sys_exit(1)
            }
        }
    }

    if paced == FRAMES {
        write(b"fb-demo: animated 240 frames, every frame vsync-paced via SYS_FB_WAIT_VBLANK\n");
    } else {
        write(b"fb-demo: animated 240 frames (some frames unpaced; check modeset wait-vblank)\n");
    }
    unsafe { sys_exit(0) }
}

fn write(bytes: &[u8]) {
    unsafe { let _ = syscall2(SYS_WRITE, bytes.as_ptr() as u64, bytes.len() as u64); }
}

#[inline(always)]
unsafe fn syscall1(num: u64, a: u64) -> u64 { syscall4(num, a, 0, 0, 0) }

#[inline(always)]
unsafe fn syscall2(num: u64, a: u64, b: u64) -> u64 { syscall4(num, a, b, 0, 0) }

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
