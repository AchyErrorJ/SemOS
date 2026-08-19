//! Framebuffer and backlight helpers for user-space programs.
//!
//! These mirror the M14 syscalls and give apps a small, safe drawing surface
//! without owning the kernel's framebuffer directly.

use crate::arch::{syscall2, syscall4, SYS_BACKLIGHT, SYS_FB_BLIT, SYS_FB_META};

/// Stable framebuffer metadata returned by [`fbinfo`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FbInfo {
    pub width: u32,
    pub height: u32,
    /// Pixels per row (may exceed `width`).
    pub stride: u32,
    pub bpp: u32,
    /// 0=unknown, 1=RGB, 2=BGR, 3=U8
    pub format: u32,
    pub byte_len: u64,
    /// Native panel resolution, or 0 if unknown.
    pub native_width: u32,
    pub native_height: u32,
}

/// Read framebuffer metadata (SYS_FB_META). Returns `None` if the kernel has
/// no framebuffer.
pub fn fbinfo() -> Option<FbInfo> {
    // Kernel writes 8 u64 words: width, height, stride, bytes_per_pixel,
    // format, byte_len, native_width, native_height.
    let mut words = [0u64; 8];
    let r = unsafe { syscall2(SYS_FB_META, words.as_mut_ptr() as u64, 64) };
    if r == u64::MAX {
        return None;
    }
    Some(FbInfo {
        width: words[0] as u32,
        height: words[1] as u32,
        stride: words[2] as u32,
        bpp: (words[3] as u32) * 8,
        format: words[4] as u32,
        byte_len: words[5],
        native_width: words[6] as u32,
        native_height: words[7] as u32,
    })
}

/// Blit a `w`×`h` pixel buffer to the framebuffer at `(x, y)` and present the
/// damaged region (SYS_FB_BLIT). Pixels are logical `0x00RRGGBB` colors; the
/// kernel converts to the hardware pixel format as needed.
///
/// Returns `true` on success.
pub fn blit(pixels: &[u32], x: usize, y: usize, w: usize, h: usize) -> bool {
    if pixels.len() < w * h {
        return false;
    }
    let xy_pack = (x as u64) | ((y as u64) << 32);
    let wh_pack = (w as u64) | ((h as u64) << 32);
    let r = unsafe {
        syscall4(
            SYS_FB_BLIT,
            xy_pack,
            wh_pack,
            pixels.as_ptr() as u64,
            (w * h) as u64,
        )
    };
    r != u64::MAX
}

/// Return the current panel brightness percent (0-100), or `None` if the
/// backlight controller is unavailable (SYS_BACKLIGHT op 0).
pub fn brightness() -> Option<u8> {
    let r = unsafe { syscall2(SYS_BACKLIGHT, 0, 0) };
    if r == u64::MAX {
        None
    } else {
        Some(r as u8)
    }
}

/// Set the panel brightness to `percent` (0-100) — SYS_BACKLIGHT op 1. The
/// kernel clamps to a safe minimum. Returns `true` on success.
pub fn set_brightness(percent: u8) -> bool {
    let r = unsafe { syscall2(SYS_BACKLIGHT, 1, percent.min(100) as u64) };
    r != u64::MAX
}
