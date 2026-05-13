//! Framebuffer Text Console
//!
//! Renders an 8x8 bitmap font into the bootloader-provided framebuffer.
//! Tracks a cursor position, scrolls when the cursor falls off the bottom,
//! and exposes a `core::fmt::Write` impl so it can back the `print!` macro.
//!
//! # Pixel format
//!
//! The bootloader exposes the framebuffer's pixel format and stride.
//! We only support `Rgb` and `Bgr` 32-bit-per-pixel formats here — that
//! covers QEMU + most modern UEFI systems. For other formats we just
//! silently skip the framebuffer write (serial output still works).
//!
//! # No allocator
//!
//! We don't have a heap, so the framebuffer state lives in a fixed
//! `Mutex<Option<FramebufferConsole>>` static. The framebuffer pointer
//! is stored as a raw `usize` to keep the type Send + Sync.

use core::fmt;
use spin::Mutex;
use bootloader_api::info::{FrameBuffer, PixelFormat};

const FONT_WIDTH: usize = 8;
const FONT_HEIGHT: usize = 8;

/// Foreground color: light gray.
const FG: u32 = 0x00C0_C0C0;
/// Background color: black.
const BG: u32 = 0x0000_0000;

/// Character cell renderer + cursor state.
///
/// The framebuffer pointer is stored as a raw usize so the struct is
/// Send + Sync (raw pointers aren't). All writes are guarded by the
/// outer Mutex, so single-threaded mutability is safe.
pub struct FramebufferConsole {
    fb_addr: usize,        // raw pointer to first byte of framebuffer
    fb_len: usize,         // total bytes
    width: usize,          // pixels
    height: usize,         // pixels
    stride: usize,         // pixels per scanline (>= width)
    bytes_per_pixel: usize,
    format: PixelFormat,
    cursor_x: usize,       // in character cells
    cursor_y: usize,
    cols: usize,           // total character columns
    rows: usize,           // total character rows
}

// SAFETY: All access goes through the outer Mutex; the raw pointer is
// only dereferenced under the lock.
unsafe impl Send for FramebufferConsole {}
unsafe impl Sync for FramebufferConsole {}

/// Global console handle. `None` until `init` is called with a framebuffer.
static CONSOLE: Mutex<Option<FramebufferConsole>> = Mutex::new(None);

// ============================================================================
// Scrollback ring buffer
// ============================================================================
//
// Captures every byte that flows through the framebuffer console (which is
// every byte that flows through `serial::_print`, since serial mirrors to
// us). This is the on-metal debug lifeline: when a fault dumps more lines
// than fit on screen, the older lines have scrolled off, but they're still
// in this buffer. PF handler can call `dump_recent_screen()` to re-render
// them after the fault clears, OR `replay_to_serial()` for QEMU runs.

const SCROLLBACK_SIZE: usize = 64 * 1024; // 64 KB — ~5-10 screens worth

/// Raw byte ring. NO Mutex — accessed via &raw mut + volatile writes,
/// which is safe because `serial::_print` already wraps everything in
/// `without_interrupts(...)`. The earlier Mutex<[u8; N]> wrapping
/// destabilized the kernel (made memcmp PF in slot 5 deterministically) —
/// suspect: spin::MutexGuard for a 64KB inner type interacts badly with
/// stack pressure or LLVM's spill choices in the println-during-syscall
/// path. Bypass the Mutex entirely.
#[repr(C, align(16))]
struct ScrollbackBuf([u8; SCROLLBACK_SIZE]);
static mut SCROLLBACK_BUF: ScrollbackBuf = ScrollbackBuf([0; SCROLLBACK_SIZE]);

/// Monotonic write index (NOT modulo'd). Index into ring = HEAD % SCROLLBACK_SIZE.
use core::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
static SCROLLBACK_HEAD: AtomicU64 = AtomicU64::new(0);

/// Append a single byte to the scrollback ring. Lock-free; safe because
/// `serial::_print` already runs inside `without_interrupts`.
fn scrollback_push(byte: u8) {
    let head = SCROLLBACK_HEAD.fetch_add(1, AtomicOrdering::Relaxed);
    let pos = (head as usize) & (SCROLLBACK_SIZE - 1);
    unsafe {
        let buf_ptr = (&raw mut SCROLLBACK_BUF) as *mut u8;
        core::ptr::write_volatile(buf_ptr.add(pos), byte);
    }
}

/// Render up to one screen-worth of the most recent scrollback content
/// directly to the framebuffer, scanning back from HEAD until we find
/// `cols * rows` worth of lines (or hit the start of recorded data).
///
/// Useful after a PF clears the screen with its dump; calling this lets
/// you see what was on screen RIGHT BEFORE the fault.
pub fn dump_recent_screen() {
    let head = SCROLLBACK_HEAD.load(AtomicOrdering::Relaxed);
    let recorded = (head as usize).min(SCROLLBACK_SIZE);

    // 8 KB ≈ one 720p screen at 8x8 chars — conservative cap so we don't
    // re-render the entire ring after a fault.
    let walk_back = recorded.min(8 * 1024);
    let start_idx = head.wrapping_sub(walk_back as u64);

    if let Some(ref mut c) = *CONSOLE.lock() {
        unsafe {
            let buf_ptr = (&raw const SCROLLBACK_BUF) as *const u8;
            let mut idx = start_idx;
            while idx != head {
                let pos = (idx as usize) & (SCROLLBACK_SIZE - 1);
                let byte = core::ptr::read_volatile(buf_ptr.add(pos));
                c.put_byte(byte);
                idx = idx.wrapping_add(1);
            }
        }
    }
}

/// Send the ENTIRE scrollback (from oldest recorded byte forward) to the
/// SERIAL port. Useful from QEMU debug paths or post-mortem to dump the
/// full recorded history. Doesn't touch the framebuffer.
pub fn replay_to_serial() {
    let head = SCROLLBACK_HEAD.load(AtomicOrdering::Relaxed);
    let recorded = (head as usize).min(SCROLLBACK_SIZE);
    let start_idx = head.wrapping_sub(recorded as u64);
    unsafe {
        let buf_ptr = (&raw const SCROLLBACK_BUF) as *const u8;
        let mut serial = crate::serial::SERIAL.lock();
        let mut idx = start_idx;
        while idx != head {
            let pos = (idx as usize) & (SCROLLBACK_SIZE - 1);
            let byte = core::ptr::read_volatile(buf_ptr.add(pos));
            serial.write_byte(byte);
            idx = idx.wrapping_add(1);
        }
    }
}

/// Initialize the framebuffer console from the bootloader's framebuffer.
pub fn init(fb: &mut FrameBuffer) {
    let info = fb.info();
    let buffer = fb.buffer_mut();
    let fb_addr = buffer.as_mut_ptr() as usize;
    let fb_len = buffer.len();
    let width = info.width;
    let height = info.height;
    let stride = info.stride;
    let bytes_per_pixel = info.bytes_per_pixel;
    let format = info.pixel_format;

    let console = FramebufferConsole {
        fb_addr,
        fb_len,
        width,
        height,
        stride,
        bytes_per_pixel,
        format,
        cursor_x: 0,
        cursor_y: 0,
        cols: width / FONT_WIDTH,
        rows: height / FONT_HEIGHT,
    };

    // Clear screen to background.
    console.clear_all();
    *CONSOLE.lock() = Some(console);
}

/// Write a string to the framebuffer console (no-op if not initialized).
pub fn write_str(s: &str) {
    if let Some(ref mut c) = *CONSOLE.lock() {
        for byte in s.bytes() {
            c.put_byte(byte);
        }
    }
}

/// `core::fmt` adapter so `format_args!` can target the framebuffer.
struct ConsoleWriter;

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Scrollback recording temporarily disabled while diagnosing whether
        // the recording itself (lock acquisition or 64KB Mutex) destabilizes
        // the kernel. Re-enable once the cause is found.
        // for byte in s.bytes() { scrollback_push(byte); }
        if let Some(ref mut c) = *CONSOLE.lock() {
            for byte in s.bytes() {
                c.put_byte(byte);
            }
        }
        Ok(())
    }
}

/// `print!`-compatible entry point used by serial::_print to mirror output.
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    let _ = ConsoleWriter.write_fmt(args);
}

impl FramebufferConsole {
    fn clear_all(&self) {
        // Fill the entire visible area with BG.
        for y in 0..self.height {
            for x in 0..self.width {
                self.put_pixel(x, y, BG);
            }
        }
    }

    fn put_pixel(&self, x: usize, y: usize, color: u32) {
        if x >= self.width || y >= self.height { return; }
        let offset = (y * self.stride + x) * self.bytes_per_pixel;
        if offset + self.bytes_per_pixel > self.fb_len { return; }
        unsafe {
            let p = (self.fb_addr + offset) as *mut u8;
            match self.format {
                PixelFormat::Rgb => {
                    // R, G, B in bytes 0,1,2.
                    *p.add(0) = ((color >> 16) & 0xFF) as u8; // R
                    *p.add(1) = ((color >>  8) & 0xFF) as u8; // G
                    *p.add(2) = ( color        & 0xFF) as u8; // B
                }
                PixelFormat::Bgr => {
                    *p.add(0) = ( color        & 0xFF) as u8; // B
                    *p.add(1) = ((color >>  8) & 0xFF) as u8; // G
                    *p.add(2) = ((color >> 16) & 0xFF) as u8; // R
                }
                _ => { /* unsupported format — skip */ }
            }
            // Bytes beyond the first 3 (alpha / padding) left as-is.
        }
    }

    fn put_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.cursor_x = 0,
            0x08 => {
                // Backspace
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                    self.draw_glyph(self.cursor_x, self.cursor_y, b' ');
                }
            }
            0x20..=0x7E => {
                // Printable ASCII
                if self.cursor_x >= self.cols {
                    self.newline();
                }
                self.draw_glyph(self.cursor_x, self.cursor_y, byte);
                self.cursor_x += 1;
            }
            _ => {
                // Replace non-printable with '?'.
                if self.cursor_x >= self.cols { self.newline(); }
                self.draw_glyph(self.cursor_x, self.cursor_y, b'?');
                self.cursor_x += 1;
            }
        }
    }

    fn newline(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += 1;
        if self.cursor_y >= self.rows {
            self.scroll();
            self.cursor_y = self.rows - 1;
        }
    }

    /// Scroll the screen up by one character row.
    fn scroll(&mut self) {
        let row_pixels = FONT_HEIGHT;
        let copy_height = self.height - row_pixels;
        // Copy lines down → up using the underlying byte buffer.
        // We move whole scanlines (stride * bpp bytes each).
        let scan_bytes = self.stride * self.bytes_per_pixel;
        unsafe {
            let base = self.fb_addr as *mut u8;
            for y in 0..copy_height {
                let src = base.add((y + row_pixels) * scan_bytes);
                let dst = base.add(y * scan_bytes);
                core::ptr::copy(src, dst, self.width * self.bytes_per_pixel);
            }
        }
        // Clear the bottom row.
        for y in copy_height..self.height {
            for x in 0..self.width {
                self.put_pixel(x, y, BG);
            }
        }
    }

    fn draw_glyph(&self, col: usize, row: usize, ch: u8) {
        let glyph = font_glyph(ch);
        let base_x = col * FONT_WIDTH;
        let base_y = row * FONT_HEIGHT;
        for (gy, row_bits) in glyph.iter().enumerate() {
            for gx in 0..FONT_WIDTH {
                // Bit 7 = leftmost pixel.
                let on = (row_bits >> (7 - gx)) & 1 != 0;
                let color = if on { FG } else { BG };
                self.put_pixel(base_x + gx, base_y + gy, color);
            }
        }
    }
}

// ============================================================================
// 8x8 bitmap font (printable ASCII subset, 0x20..=0x7E)
// ============================================================================
//
// The font is small but legible. Each glyph is 8 rows × 8 columns,
// MSB-first within each byte. A blank pattern is returned for any
// character outside the supported range.

fn font_glyph(ch: u8) -> [u8; 8] {
    if !(0x20..=0x7E).contains(&ch) {
        return [0; 8];
    }
    let idx = (ch - 0x20) as usize;
    FONT_8X8[idx]
}

#[rustfmt::skip]
const FONT_8X8: [[u8; 8]; 95] = [
    // 0x20 ' '
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 0x21 '!'
    [0x18,0x18,0x18,0x18,0x00,0x00,0x18,0x00],
    // 0x22 '"'
    [0x36,0x36,0x00,0x00,0x00,0x00,0x00,0x00],
    // 0x23 '#'
    [0x36,0x36,0x7F,0x36,0x7F,0x36,0x36,0x00],
    // 0x24 '$'
    [0x18,0x3E,0x60,0x3C,0x06,0x7C,0x18,0x00],
    // 0x25 '%'
    [0x00,0x63,0x66,0x0C,0x18,0x33,0x63,0x00],
    // 0x26 '&'
    [0x1C,0x36,0x1C,0x6E,0x3B,0x33,0x6E,0x00],
    // 0x27 '\''
    [0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00],
    // 0x28 '('
    [0x0C,0x18,0x30,0x30,0x30,0x18,0x0C,0x00],
    // 0x29 ')'
    [0x30,0x18,0x0C,0x0C,0x0C,0x18,0x30,0x00],
    // 0x2A '*'
    [0x00,0x66,0x3C,0xFF,0x3C,0x66,0x00,0x00],
    // 0x2B '+'
    [0x00,0x18,0x18,0x7E,0x18,0x18,0x00,0x00],
    // 0x2C ','
    [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x30],
    // 0x2D '-'
    [0x00,0x00,0x00,0x7E,0x00,0x00,0x00,0x00],
    // 0x2E '.'
    [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x00],
    // 0x2F '/'
    [0x03,0x06,0x0C,0x18,0x30,0x60,0x40,0x00],
    // 0x30 '0'
    [0x3E,0x63,0x67,0x6F,0x7B,0x73,0x3E,0x00],
    // 0x31 '1'
    [0x18,0x38,0x18,0x18,0x18,0x18,0x7E,0x00],
    // 0x32 '2'
    [0x3C,0x66,0x06,0x0C,0x18,0x30,0x7E,0x00],
    // 0x33 '3'
    [0x3C,0x66,0x06,0x1C,0x06,0x66,0x3C,0x00],
    // 0x34 '4'
    [0x0C,0x1C,0x3C,0x6C,0x7E,0x0C,0x0C,0x00],
    // 0x35 '5'
    [0x7E,0x60,0x7C,0x06,0x06,0x66,0x3C,0x00],
    // 0x36 '6'
    [0x1C,0x30,0x60,0x7C,0x66,0x66,0x3C,0x00],
    // 0x37 '7'
    [0x7E,0x06,0x0C,0x18,0x30,0x30,0x30,0x00],
    // 0x38 '8'
    [0x3C,0x66,0x66,0x3C,0x66,0x66,0x3C,0x00],
    // 0x39 '9'
    [0x3C,0x66,0x66,0x3E,0x06,0x0C,0x38,0x00],
    // 0x3A ':'
    [0x00,0x18,0x18,0x00,0x00,0x18,0x18,0x00],
    // 0x3B ';'
    [0x00,0x18,0x18,0x00,0x00,0x18,0x18,0x30],
    // 0x3C '<'
    [0x06,0x0C,0x18,0x30,0x18,0x0C,0x06,0x00],
    // 0x3D '='
    [0x00,0x00,0x7E,0x00,0x7E,0x00,0x00,0x00],
    // 0x3E '>'
    [0x60,0x30,0x18,0x0C,0x18,0x30,0x60,0x00],
    // 0x3F '?'
    [0x3C,0x66,0x0C,0x18,0x18,0x00,0x18,0x00],
    // 0x40 '@'
    [0x3C,0x66,0x6E,0x6E,0x60,0x62,0x3C,0x00],
    // 0x41 'A'
    [0x18,0x3C,0x66,0x66,0x7E,0x66,0x66,0x00],
    // 0x42 'B'
    [0x7C,0x66,0x66,0x7C,0x66,0x66,0x7C,0x00],
    // 0x43 'C'
    [0x3C,0x66,0x60,0x60,0x60,0x66,0x3C,0x00],
    // 0x44 'D'
    [0x78,0x6C,0x66,0x66,0x66,0x6C,0x78,0x00],
    // 0x45 'E'
    [0x7E,0x60,0x60,0x78,0x60,0x60,0x7E,0x00],
    // 0x46 'F'
    [0x7E,0x60,0x60,0x78,0x60,0x60,0x60,0x00],
    // 0x47 'G'
    [0x3C,0x66,0x60,0x6E,0x66,0x66,0x3E,0x00],
    // 0x48 'H'
    [0x66,0x66,0x66,0x7E,0x66,0x66,0x66,0x00],
    // 0x49 'I'
    [0x3C,0x18,0x18,0x18,0x18,0x18,0x3C,0x00],
    // 0x4A 'J'
    [0x1E,0x0C,0x0C,0x0C,0x0C,0x6C,0x38,0x00],
    // 0x4B 'K'
    [0x66,0x6C,0x78,0x70,0x78,0x6C,0x66,0x00],
    // 0x4C 'L'
    [0x60,0x60,0x60,0x60,0x60,0x60,0x7E,0x00],
    // 0x4D 'M'
    [0x63,0x77,0x7F,0x6B,0x63,0x63,0x63,0x00],
    // 0x4E 'N'
    [0x66,0x76,0x7E,0x7E,0x6E,0x66,0x66,0x00],
    // 0x4F 'O'
    [0x3C,0x66,0x66,0x66,0x66,0x66,0x3C,0x00],
    // 0x50 'P'
    [0x7C,0x66,0x66,0x7C,0x60,0x60,0x60,0x00],
    // 0x51 'Q'
    [0x3C,0x66,0x66,0x66,0x66,0x3C,0x0E,0x00],
    // 0x52 'R'
    [0x7C,0x66,0x66,0x7C,0x78,0x6C,0x66,0x00],
    // 0x53 'S'
    [0x3C,0x66,0x60,0x3C,0x06,0x66,0x3C,0x00],
    // 0x54 'T'
    [0x7E,0x18,0x18,0x18,0x18,0x18,0x18,0x00],
    // 0x55 'U'
    [0x66,0x66,0x66,0x66,0x66,0x66,0x3C,0x00],
    // 0x56 'V'
    [0x66,0x66,0x66,0x66,0x66,0x3C,0x18,0x00],
    // 0x57 'W'
    [0x63,0x63,0x63,0x6B,0x7F,0x77,0x63,0x00],
    // 0x58 'X'
    [0x66,0x66,0x3C,0x18,0x3C,0x66,0x66,0x00],
    // 0x59 'Y'
    [0x66,0x66,0x66,0x3C,0x18,0x18,0x18,0x00],
    // 0x5A 'Z'
    [0x7E,0x06,0x0C,0x18,0x30,0x60,0x7E,0x00],
    // 0x5B '['
    [0x3C,0x30,0x30,0x30,0x30,0x30,0x3C,0x00],
    // 0x5C '\\'
    [0x60,0x30,0x18,0x0C,0x06,0x03,0x01,0x00],
    // 0x5D ']'
    [0x3C,0x0C,0x0C,0x0C,0x0C,0x0C,0x3C,0x00],
    // 0x5E '^'
    [0x18,0x3C,0x66,0x00,0x00,0x00,0x00,0x00],
    // 0x5F '_'
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xFF],
    // 0x60 '`'
    [0x18,0x18,0x0C,0x00,0x00,0x00,0x00,0x00],
    // 0x61 'a'
    [0x00,0x00,0x3C,0x06,0x3E,0x66,0x3E,0x00],
    // 0x62 'b'
    [0x60,0x60,0x7C,0x66,0x66,0x66,0x7C,0x00],
    // 0x63 'c'
    [0x00,0x00,0x3C,0x66,0x60,0x66,0x3C,0x00],
    // 0x64 'd'
    [0x06,0x06,0x3E,0x66,0x66,0x66,0x3E,0x00],
    // 0x65 'e'
    [0x00,0x00,0x3C,0x66,0x7E,0x60,0x3C,0x00],
    // 0x66 'f'
    [0x1C,0x36,0x30,0x7C,0x30,0x30,0x30,0x00],
    // 0x67 'g'
    [0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x7C],
    // 0x68 'h'
    [0x60,0x60,0x7C,0x66,0x66,0x66,0x66,0x00],
    // 0x69 'i'
    [0x18,0x00,0x38,0x18,0x18,0x18,0x3C,0x00],
    // 0x6A 'j'
    [0x06,0x00,0x06,0x06,0x06,0x06,0x66,0x3C],
    // 0x6B 'k'
    [0x60,0x60,0x66,0x6C,0x78,0x6C,0x66,0x00],
    // 0x6C 'l'
    [0x38,0x18,0x18,0x18,0x18,0x18,0x3C,0x00],
    // 0x6D 'm'
    [0x00,0x00,0x66,0x7F,0x7F,0x6B,0x63,0x00],
    // 0x6E 'n'
    [0x00,0x00,0x7C,0x66,0x66,0x66,0x66,0x00],
    // 0x6F 'o'
    [0x00,0x00,0x3C,0x66,0x66,0x66,0x3C,0x00],
    // 0x70 'p'
    [0x00,0x00,0x7C,0x66,0x66,0x7C,0x60,0x60],
    // 0x71 'q'
    [0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x06],
    // 0x72 'r'
    [0x00,0x00,0x7C,0x66,0x60,0x60,0x60,0x00],
    // 0x73 's'
    [0x00,0x00,0x3E,0x60,0x3C,0x06,0x7C,0x00],
    // 0x74 't'
    [0x18,0x7E,0x18,0x18,0x18,0x1A,0x0C,0x00],
    // 0x75 'u'
    [0x00,0x00,0x66,0x66,0x66,0x66,0x3E,0x00],
    // 0x76 'v'
    [0x00,0x00,0x66,0x66,0x66,0x3C,0x18,0x00],
    // 0x77 'w'
    [0x00,0x00,0x63,0x6B,0x7F,0x7F,0x36,0x00],
    // 0x78 'x'
    [0x00,0x00,0x66,0x3C,0x18,0x3C,0x66,0x00],
    // 0x79 'y'
    [0x00,0x00,0x66,0x66,0x66,0x3E,0x06,0x7C],
    // 0x7A 'z'
    [0x00,0x00,0x7E,0x0C,0x18,0x30,0x7E,0x00],
    // 0x7B '{'
    [0x0E,0x18,0x18,0x70,0x18,0x18,0x0E,0x00],
    // 0x7C '|'
    [0x18,0x18,0x18,0x18,0x18,0x18,0x18,0x00],
    // 0x7D '}'
    [0x70,0x18,0x18,0x0E,0x18,0x18,0x70,0x00],
    // 0x7E '~'
    [0x00,0x00,0x76,0xDC,0x00,0x00,0x00,0x00],
];
