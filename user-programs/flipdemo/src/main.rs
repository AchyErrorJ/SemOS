//! flipdemo.elf — Rung C tear-free page-flip demo (Ring 3).
//!
//! Claims the screen (SYS_FB_CLAIM), then bounces a bright bar across the
//! panel for ~10 seconds. Each frame renders into a user-owned full-frame
//! buffer, blits it into the kernel's *hidden* scanout buffer with one
//! SYS_FB_BLIT, and swaps buffers with SYS_FB_FLIP — the DSPSURF write
//! latches at the next vblank, so presents are atomic and cannot tear.
//!
//! If the flip syscall is refused (machine not using stolen-relative plane
//! addressing, or stolen memory too small), the demo falls back to
//! vblank-paced single-buffer blits and says so. ESC quits early.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_WRITE: u64 = 0;
const SYS_EXIT: u64 = 2;
const SYS_FB_META: u64 = 128;
const SYS_FB_BLIT: u64 = 129;
const SYS_FB_WAIT_VBLANK: u64 = 131;
const SYS_KB_POLL: u64 = 139;
const SYS_FB_CLAIM: u64 = 140;
const SYS_FB_FLIP: u64 = 141;

const MAX_W: usize = 1920;
const MAX_H: usize = 1080;
const FRAMES: u32 = 600;
const BAR_W: usize = 40;

// Raw key event record: bit 31 = pressed, bits 6:0 = set-1 scancode.
const KEY_PRESSED: u32 = 1 << 31;
const SCANCODE_ESC: u32 = 0x01;

static mut FRAME: [u32; MAX_W * MAX_H] = [0; MAX_W * MAX_H];

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    let mut meta = [0u64; 8];
    let rc = unsafe { syscall2(SYS_FB_META, meta.as_mut_ptr() as u64, 64) };
    if rc == u64::MAX {
        write(b"flipdemo: framebuffer metadata unavailable\n");
        unsafe { sys_exit(1) }
    }
    let w = (meta[0] as usize).min(MAX_W);
    let h = (meta[1] as usize).min(MAX_H);
    if w < 640 || h < 480 {
        write(b"flipdemo: framebuffer too small\n");
        unsafe { sys_exit(1) }
    }

    if unsafe { syscall1(SYS_FB_CLAIM, 1) } == u64::MAX {
        write(b"flipdemo: SYS_FB_CLAIM failed\n");
        unsafe { sys_exit(1) }
    }

    let mut flipped: u32 = 0;
    let mut flip_broken = false;
    let mut quit = false;
    let mut x: usize = 0;
    let mut dir_right = true;
    let mut keys = [0u32; 16];

    for _ in 0..FRAMES {
        if quit {
            break;
        }

        // ESC (or any ctrl+c) quits.
        let n = unsafe { syscall2(SYS_KB_POLL, keys.as_mut_ptr() as u64, 64) };
        if n != u64::MAX {
            let mut i = 0;
            while i < n as usize && i < 16 {
                let ev = keys[i];
                if ev & KEY_PRESSED != 0 && (ev & 0x7F) == SCANCODE_ESC {
                    quit = true;
                }
                i += 1;
            }
        }

        // Render the frame: dark gradient background + bouncing bar.
        let frame = unsafe { &mut *core::ptr::addr_of_mut!(FRAME) };
        let mut py = 0;
        while py < h {
            let shade = 0x10 + ((py * 24 / h) as u32); // 0x10..0x28 blue channel
            let bg = 0x0010_2000 | shade;
            let mut px = 0;
            while px < w {
                let in_bar = px >= x && px < x + BAR_W;
                frame[py * w + px] = if in_bar { 0x00F0_F0F0 } else { bg };
                px += 1;
            }
            py += 1;
        }

        // Bounce: 17 px/frame (~1020 px/s at 60 fps) so edges sweep fast
        // enough that any tearing would be obvious.
        let max_x = w - BAR_W;
        if dir_right {
            x = if x + 17 > max_x { max_x } else { x + 17 };
            if x == max_x {
                dir_right = false;
            }
        } else {
            x = x.saturating_sub(17);
            if x == 0 {
                dir_right = true;
            }
        }

        // Present: blit into the hidden buffer, then flip.
        let xy = 0u64;
        let wh = (w as u64) | ((h as u64) << 32);
        let rc = unsafe { syscall4(SYS_FB_BLIT, xy, wh, frame.as_ptr() as u64, (w * h) as u64) };
        if rc == u64::MAX {
            write(b"flipdemo: SYS_FB_BLIT failed\n");
            unsafe { syscall1(SYS_FB_CLAIM, 0) };
            unsafe { sys_exit(1) }
        }
        if !flip_broken {
            let frc = unsafe { syscall1(SYS_FB_FLIP, 0) };
            if frc == 0 {
                flipped += 1;
            } else {
                flip_broken = true;
                write(b"flipdemo: SYS_FB_FLIP unavailable - falling back to vblank pacing\n");
            }
        }
        if flip_broken {
            let _ = unsafe { syscall1(SYS_FB_WAIT_VBLANK, 0) };
        }
    }

    unsafe { syscall1(SYS_FB_CLAIM, 0) };

    if flipped > 0 {
        write(b"flipdemo: ran with hardware page flips (tear-free double buffering)\n");
    } else {
        write(b"flipdemo: ran with vblank-paced blits only (no flip)\n");
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
