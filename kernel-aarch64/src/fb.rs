//! Framebuffer console — the kernel's only voice on a Mac.
//!
//! On a MacBook the hardware UART our `serial.rs` drives is not on any port you
//! can plug a cable into, and m1n1's serial console is a **USB gadget** that
//! needs a second machine on the other end. With one machine, a UART-only
//! kernel boots completely blind. m1n1 has already brought the display up and
//! printed its own log to it, and it passes the framebuffer on in the device
//! tree — so we render text there too, and every existing `uart_str` caller
//! (including the panic handler) shows up on screen for free.
//!
//! Deliberately dumb: no allocator, no double-buffer, no scrollback. It has to
//! work when nothing else does.
//!
//! **Pixel formats.** 32-bit only, and Apple is the reason both are here:
//! `x8r8g8b8` is the common case, but Apple panels commonly run 10 bits per
//! channel (`x2r10g10b10`), where writing 8-bit-packed pixels produces a dim,
//! wrongly-coloured mess rather than an obvious failure.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::fdt::Framebuffer;
use crate::font::FONT8X8;

const GLYPH_W: u32 = 8;
const GLYPH_H: u32 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    /// 8 bits per channel, blue in the low byte.
    X8R8G8B8,
    /// Apple: 10 bits per channel.
    X2R10G10B10,
}

struct Console {
    base: u64,
    width: u32,
    height: u32,
    stride: u32,
    format: Format,
    /// Glyphs are 8x8; on a Retina panel that is unreadable, so scale up.
    scale: u32,
    cur_x: u32,
    cur_y: u32,
}

static mut CON: Console = Console {
    base: 0,
    width: 0,
    height: 0,
    stride: 0,
    format: Format::X8R8G8B8,
    scale: 1,
    cur_x: 0,
    cur_y: 0,
};
static READY: AtomicBool = AtomicBool::new(false);

const FG: (u8, u8, u8) = (0xC8, 0xD0, 0xD8);
const BG: (u8, u8, u8) = (0x08, 0x0A, 0x0C);

/// Is the framebuffer console live?
pub fn present() -> bool {
    READY.load(Ordering::Acquire)
}

fn parse_format(f: &str) -> Option<Format> {
    match f {
        "x8r8g8b8" | "a8r8g8b8" | "x8b8g8r8" => Some(Format::X8R8G8B8),
        "x2r10g10b10" | "a2r10g10b10" => Some(Format::X2R10G10B10),
        _ => None,
    }
}

#[inline]
fn pack(fmt: Format, c: (u8, u8, u8)) -> u32 {
    let (r, g, b) = c;
    match fmt {
        Format::X8R8G8B8 => ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
        // Widen 8-bit to 10-bit by replicating the high bits, so 0xFF maps to
        // full-scale 0x3FF rather than 0x3FC.
        Format::X2R10G10B10 => {
            let w = |v: u8| -> u32 {
                let v = v as u32;
                (v << 2) | (v >> 6)
            };
            (w(r) << 20) | (w(g) << 10) | w(b)
        }
    }
}

#[inline]
unsafe fn put_pixel(con: &Console, x: u32, y: u32, v: u32) {
    if x >= con.width || y >= con.height {
        return;
    }
    let addr = con.base + (y as u64) * (con.stride as u64) + (x as u64) * 4;
    core::ptr::write_volatile(addr as *mut u32, v);
}

/// Adopt the framebuffer the device tree describes and clear it.
///
/// Must run once the framebuffer is reachable (before the MMU it is physical
/// and directly writable). During bring-up this is intentionally forgiving:
/// an unrecognized `format` string falls back to the common 8-bit layout
/// rather than leaving the screen dark, because a wrongly-coloured console is
/// still infinitely more debuggable than a silent one.
pub unsafe fn init(fb: &Framebuffer) -> bool {
    // A framebuffer with no base or no dimensions is unusable no matter what.
    if fb.base == 0 || fb.width == 0 || fb.height == 0 {
        return false;
    }
    let format = parse_format(fb.format_str()).unwrap_or(Format::X8R8G8B8);

    // Prefer the stride the tree gives; if it is nonsense, assume packed 32bpp.
    let stride = if fb.stride >= fb.width * 4 {
        fb.stride
    } else {
        fb.width * 4
    };

    let scale = (fb.height / 400).clamp(1, 4);

    CON = Console {
        base: fb.base,
        width: fb.width,
        height: fb.height,
        stride,
        format,
        scale,
        cur_x: 0,
        cur_y: 0,
    };

    READY.store(true, Ordering::Release);

    // Bring-up "I am alive" flash: paint the WHOLE screen a bright, unmistakable
    // colour so a successful m1n1 -> kernel handoff is visible even before a
    // single glyph is drawn. If the screen ever leaves the m1n1 logo and turns
    // solid teal, the kernel is running and the framebuffer is real.
    let alive = pack(format, (0x00, 0xB0, 0xC0));
    for y in 0..CON.height {
        for x in 0..CON.width {
            put_pixel(&CON, x, y, alive);
        }
    }
    // Then settle to the console background so text is readable.
    let bg = pack(format, BG);
    for y in 0..CON.height {
        for x in 0..CON.width {
            put_pixel(&CON, x, y, bg);
        }
    }

    true
}

/// Flood the entire framebuffer with one colour. Used by the exception and
/// panic handlers so a fault is impossible to miss on a headless Mac. No-op if
/// the console was never brought up.
pub fn flood(r: u8, g: u8, b: u8) {
    if !present() {
        return;
    }
    unsafe {
        let con = &*core::ptr::addr_of!(CON);
        let v = pack(con.format, (r, g, b));
        for y in 0..con.height {
            for x in 0..con.width {
                put_pixel(con, x, y, v);
            }
        }
    }
}

/// Scroll up one text line.
unsafe fn scroll(con: &mut Console) {
    let line = GLYPH_H * con.scale;
    let bg = pack(con.format, BG);
    let row_bytes = con.stride as u64;

    // Move every scanline up by one glyph row. Byte-wise so a stride that is
    // not a multiple of 4 still works.
    let shift = (line as u64) * row_bytes;
    let total = (con.height as u64) * row_bytes;
    let src = con.base + shift;
    let dst = con.base;
    let count = total.saturating_sub(shift);
    core::ptr::copy(src as *const u8, dst as *mut u8, count as usize);

    // Clear the freed line at the bottom.
    for y in (con.height - line)..con.height {
        for x in 0..con.width {
            put_pixel(con, x, y, bg);
        }
    }
    con.cur_y = con.height - line;
}

unsafe fn newline(con: &mut Console) {
    con.cur_x = 0;
    let line = GLYPH_H * con.scale;
    if con.cur_y + 2 * line > con.height {
        scroll(con);
    } else {
        con.cur_y += line;
    }
}

/// Render one byte. `\n` and `\r` behave as on a terminal; anything outside
/// printable ASCII is drawn as a blank so a stray byte cannot corrupt the layout.
pub fn putc(b: u8) {
    if !present() {
        return;
    }
    unsafe {
        let con = &mut *core::ptr::addr_of_mut!(CON);

        match b {
            b'\n' => {
                newline(con);
                return;
            }
            b'\r' => {
                con.cur_x = 0;
                return;
            }
            _ => {}
        }

        let cw = GLYPH_W * con.scale;
        if con.cur_x + cw > con.width {
            newline(con);
        }

        let glyph = if (0x20..0x7F).contains(&b) {
            FONT8X8[(b - 0x20) as usize]
        } else {
            [0u8; 8]
        };

        let fg = pack(con.format, FG);
        let bg = pack(con.format, BG);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8u32 {
                // font8x8 packs bit 0 as the leftmost pixel.
                let on = (bits >> col) & 1 != 0;
                let v = if on { fg } else { bg };
                for sy in 0..con.scale {
                    for sx in 0..con.scale {
                        put_pixel(
                            con,
                            con.cur_x + col * con.scale + sx,
                            con.cur_y + (row as u32) * con.scale + sy,
                            v,
                        );
                    }
                }
            }
        }
        con.cur_x += cw;
    }
}

/// Read a pixel back. Used by the boot self-test to prove the console actually
/// reached the buffer — on real hardware the alternative is trusting a screen
/// nobody is looking at.
pub fn read_pixel(x: u32, y: u32) -> u32 {
    unsafe {
        let con = &*core::ptr::addr_of!(CON);
        if !present() || x >= con.width || y >= con.height {
            return 0;
        }
        let addr = con.base + (y as u64) * (con.stride as u64) + (x as u64) * 4;
        core::ptr::read_volatile(addr as *const u32)
    }
}

/// `(width, height, scale)` — for boot logs.
pub fn geometry() -> (u32, u32, u32) {
    unsafe {
        let con = &*core::ptr::addr_of!(CON);
        (con.width, con.height, con.scale)
    }
}
