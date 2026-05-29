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

/// Integer upscale for the 8x8 bitmap font, plus a per-row gap, so boot/console
/// text is legible on a high-res framebuffer instead of cramped 8px cells. At
/// 1280x720 this gives ~80x36 character cells (was 160x90). Bump `FONT_SCALE`
/// for bigger text; `LINE_GAP` adds breathing room between rows.
const FONT_SCALE: usize = 2;
const LINE_GAP: usize = 4;
/// Character cell size in pixels (glyph box + gap).
const CELL_W: usize = FONT_WIDTH * FONT_SCALE;
const CELL_H: usize = FONT_HEIGHT * FONT_SCALE + LINE_GAP;

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
// M6 — Framebuffer drawing API
// ============================================================================
//
// # Detected pixel format (live, from `FrameBufferInfo`)
//
// On QEMU's `-vga std` / bootloader-0.11 default the framebuffer reports:
//   pixel_format = PixelFormat::Bgr   (blue in byte 0, green byte 1, red byte 2)
//   bytes_per_pixel = 4               (32 bpp; byte 3 is unused padding/alpha)
//   stride          = width (often == width, but we never assume; we read it)
//
// We DO NOT hard-code this — `init` records whatever the bootloader hands us
// and the `Color`/`rgb()` packer + `FbSurface::write_pixel` switch on the
// live `PixelFormat`. The constants above are just what we observe in QEMU.
//
// # Presentation model
//
// We draw DIRECTLY into the bootloader framebuffer (write-through). A full
// shadow back buffer would cost width*height*4 bytes (~3.5 MiB at 1024x768)
// of static BSS or heap; on this kernel that's heavier than the value it
// buys for the current single-surface use, so we present directly and instead
// track a *damage rectangle*. `fb_present()` is the commit point: it reads
// back the union of all dirtied regions to flush write-combining-style
// caches (a volatile read per dirty corner) and then clears the damage rect.
// Apps that want tear-free updates call the primitives, then `fb_present()`.
// If a true back buffer is later needed for compositing, `FbSurface` already
// centralizes every pixel write so it can be retargeted in one place.

/// A packed 32-bit color in the framebuffer's native byte order is produced
/// on demand by `FbSurface::write_pixel`; callers pass logical 0x00RRGGBB
/// values (this `Color`) and the surface packs per the detected format.
pub type Color = u32;

/// Pack an (r, g, b) triple into the logical 0x00RRGGBB color the drawing
/// API consumes. The surface re-orders bytes to BGR/RGB as needed at write
/// time, so callers always speak RGB here regardless of hardware order.
#[inline]
pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// Drawing surface: a thin view over the bootloader framebuffer bytes plus
/// the geometry/format needed to address pixels. Cloned (by value) out of
/// the global on each drawing call so we never hold the console lock while
/// touching pixels.
#[derive(Clone, Copy)]
pub struct FbSurface {
    addr: usize,
    len: usize,
    width: usize,
    height: usize,
    stride: usize,
    bpp: usize,
    format: PixelFormat,
}

// SAFETY: same contract as FramebufferConsole — pointer only dereferenced
// while the SURFACE mutex is held, single-threaded boot path.
unsafe impl Send for FbSurface {}
unsafe impl Sync for FbSurface {}

/// Global drawing surface. Populated by `init` alongside the console.
static SURFACE: Mutex<Option<FbSurface>> = Mutex::new(None);

/// Damage rectangle accumulated since the last `fb_present()`. Stored as
/// (x0, y0, x1, y1) half-open; `None` means "nothing dirty".
static DAMAGE: Mutex<Option<(usize, usize, usize, usize)>> = Mutex::new(None);

impl FbSurface {
    /// Framebuffer dimensions in pixels.
    #[inline]
    pub fn dimensions(&self) -> (usize, usize) { (self.width, self.height) }

    /// Write one pixel, packing `color` (logical 0x00RRGGBB) into the
    /// detected native byte order. Bounds-checked; OOB is a silent no-op so
    /// every primitive can lean on this for clipping safety.
    #[inline]
    fn write_pixel(&self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height { return; }
        let offset = (y * self.stride + x) * self.bpp;
        if offset + self.bpp > self.len { return; }
        unsafe {
            let p = (self.addr + offset) as *mut u8;
            match self.format {
                PixelFormat::Rgb => {
                    core::ptr::write_volatile(p.add(0), ((color >> 16) & 0xFF) as u8);
                    core::ptr::write_volatile(p.add(1), ((color >> 8) & 0xFF) as u8);
                    core::ptr::write_volatile(p.add(2), (color & 0xFF) as u8);
                }
                PixelFormat::Bgr => {
                    core::ptr::write_volatile(p.add(0), (color & 0xFF) as u8);
                    core::ptr::write_volatile(p.add(1), ((color >> 8) & 0xFF) as u8);
                    core::ptr::write_volatile(p.add(2), ((color >> 16) & 0xFF) as u8);
                }
                _ => { /* unsupported format — skip */ }
            }
        }
    }

    /// Read one pixel back as a logical 0x00RRGGBB color. Used by the DEMO
    /// to verify draws in a headless run. OOB reads return 0.
    #[inline]
    fn read_pixel(&self, x: usize, y: usize) -> Color {
        if x >= self.width || y >= self.height { return 0; }
        let offset = (y * self.stride + x) * self.bpp;
        if offset + self.bpp > self.len { return 0; }
        unsafe {
            let p = (self.addr + offset) as *const u8;
            let b0 = core::ptr::read_volatile(p.add(0)) as u32;
            let b1 = core::ptr::read_volatile(p.add(1)) as u32;
            let b2 = core::ptr::read_volatile(p.add(2)) as u32;
            match self.format {
                PixelFormat::Rgb => (b0 << 16) | (b1 << 8) | b2,
                PixelFormat::Bgr => (b2 << 16) | (b1 << 8) | b0,
                _ => 0,
            }
        }
    }
}

/// Union `rect` (x, y, w, h) into the global damage rectangle, clipped to
/// the surface bounds.
fn mark_damage(s: &FbSurface, x: usize, y: usize, w: usize, h: usize) {
    if w == 0 || h == 0 { return; }
    let x0 = x.min(s.width);
    let y0 = y.min(s.height);
    let x1 = (x + w).min(s.width);
    let y1 = (y + h).min(s.height);
    if x0 >= x1 || y0 >= y1 { return; }
    let mut d = DAMAGE.lock();
    *d = Some(match *d {
        None => (x0, y0, x1, y1),
        Some((ox0, oy0, ox1, oy1)) => (
            ox0.min(x0), oy0.min(y0), ox1.max(x1), oy1.max(y1),
        ),
    });
}

/// Snapshot the global surface, or `None` if the framebuffer isn't up.
fn surface() -> Option<FbSurface> { *SURFACE.lock() }

/// Fill an axis-aligned rectangle with a solid color. Clipped to the
/// framebuffer; never writes out of bounds.
pub fn fb_fill_rect(x: usize, y: usize, w: usize, h: usize, color: Color) {
    let s = match surface() { Some(s) => s, None => return };
    let x1 = (x + w).min(s.width);
    let y1 = (y + h).min(s.height);
    let x = x.min(s.width);
    let y = y.min(s.height);
    for py in y..y1 {
        for px in x..x1 {
            s.write_pixel(px, py, color);
        }
    }
    mark_damage(&s, x, y, x1.saturating_sub(x), y1.saturating_sub(y));
}

/// Blit a row-major `src` image of size `w`x`h` (logical 0x00RRGGBB pixels)
/// to (x, y). Clipped to the framebuffer; source pixels that would land OOB
/// are skipped. `src` must have at least `w * h` entries (extra ignored).
pub fn fb_blit(src: &[Color], x: usize, y: usize, w: usize, h: usize) {
    let s = match surface() { Some(s) => s, None => return };
    if src.len() < w * h { return; }
    for row in 0..h {
        let dy = y + row;
        if dy >= s.height { break; }
        for col in 0..w {
            let dx = x + col;
            if dx >= s.width { break; }
            s.write_pixel(dx, dy, src[row * w + col]);
        }
    }
    mark_damage(&s, x, y, w, h);
}

/// Scroll the whole framebuffer by (dx, dy) pixels. Positive dy scrolls
/// content UP by dy (toward the origin); positive dx scrolls content LEFT.
/// Vacated edges are filled black. Negative deltas scroll the other way.
/// Whole surface is marked damaged.
///
/// Row direction is chosen to be overlap-safe for vertical scrolls (the
/// common console/TTY case). Horizontal scrolls with `dx < 0` that overlap
/// can read already-written source pixels within a row; vertical-only and
/// `dx >= 0` scrolls are exact. Callers needing exact arbitrary 2D shifts
/// should blit through a scratch buffer.
pub fn fb_scroll(dx: isize, dy: isize) {
    let s = match surface() { Some(s) => s, None => return };
    let w = s.width as isize;
    let h = s.height as isize;
    // Iterate destination pixels; pull from (dst + delta) source. For dy>0
    // (scroll up) iterate rows ascending so we never overwrite unread source;
    // for dy<0 iterate descending. Vacated pixels are filled BG.
    let copy_row = |dst_y: isize| {
        let src_y = dst_y + dy;
        for dst_x in 0..w {
            let src_x = dst_x + dx;
            let color = if src_x >= 0 && src_x < w && src_y >= 0 && src_y < h {
                s.read_pixel(src_x as usize, src_y as usize)
            } else {
                BG
            };
            s.write_pixel(dst_x as usize, dst_y as usize, color);
        }
    };
    if dy >= 0 {
        for dst_y in 0..h { copy_row(dst_y); }
    } else {
        for dst_y in (0..h).rev() { copy_row(dst_y); }
    }
    mark_damage(&s, 0, 0, s.width, s.height);
}

/// Scroll a sub-rectangle `(x, y, w, h)` UP by `dy` pixels: kept content moves
/// toward the region's top and the vacated bottom `dy` rows are filled `bg`.
/// Clipped to the framebuffer. Used by the TTY console so advancing one text
/// line leaves the rest of the screen untouched (unlike whole-screen `fb_scroll`).
pub fn fb_scroll_region(x: usize, y: usize, w: usize, h: usize, dy: usize, bg: Color) {
    let s = match surface() { Some(s) => s, None => return };
    let x1 = (x + w).min(s.width);
    let y1 = (y + h).min(s.height);
    if x >= x1 || y >= y1 { return; }
    let xw = x1 - x;
    // If we'd scroll out everything we keep, just clear the region.
    let kept_end = y1.saturating_sub(dy);
    if dy == 0 || kept_end <= y {
        fb_fill_rect(x, y, xw, y1 - y, bg);
        mark_damage(&s, x, y, xw, y1 - y);
        return;
    }
    // Move rows up: dst row r ← src row r + dy (ascending: never overwrite
    // a source row before it's read, since dst < src).
    for r in y..kept_end {
        let src = r + dy;
        for c in x..x1 {
            s.write_pixel(c, r, s.read_pixel(c, src));
        }
    }
    // Clear the vacated bottom dy rows.
    fb_fill_rect(x, kept_end, xw, y1 - kept_end, bg);
    mark_damage(&s, x, y, xw, y1 - y);
}

/// Commit point for the damage model. With direct-to-framebuffer rendering
/// there's no separate buffer to copy, so `fb_present` flushes the dirtied
/// region (a volatile readback of its corners forces ordering against any
/// write-combining the MMIO mapping might do) and then clears the damage
/// rect. Returns the rect that was presented, if any.
pub fn fb_present() -> Option<(usize, usize, usize, usize)> {
    let s = surface()?;
    let rect = DAMAGE.lock().take()?;
    let (x0, y0, x1, y1) = rect;
    // Touch the four corners of the damage rect to flush ordering.
    let _ = s.read_pixel(x0, y0);
    let _ = s.read_pixel(x1.saturating_sub(1), y0);
    let _ = s.read_pixel(x0, y1.saturating_sub(1));
    let _ = s.read_pixel(x1.saturating_sub(1), y1.saturating_sub(1));
    Some(rect)
}

/// Read back a single pixel (logical 0x00RRGGBB). For verification/DEMO use.
/// Returns 0 if the framebuffer isn't up or the coords are OOB.
pub fn fb_read_pixel(x: usize, y: usize) -> Color {
    match surface() { Some(s) => s.read_pixel(x, y), None => 0 }
}

/// Framebuffer dimensions in pixels, or (0, 0) if not initialized.
pub fn fb_dimensions() -> (usize, usize) {
    match surface() { Some(s) => s.dimensions(), None => (0, 0) }
}

/// Detected format descriptor for diagnostics: (bpp, stride, is_rgb).
pub fn fb_format() -> (usize, usize, bool) {
    match surface() {
        Some(s) => (s.bpp, s.stride, matches!(s.format, PixelFormat::Rgb)),
        None => (0, 0, false),
    }
}

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

/// Current monotonic write head into the scrollback ring (not modulo'd).
/// Exposed so the panic-dump path can snapshot the ring without copying it
/// into a 64 KiB intermediate buffer.
pub fn scrollback_head() -> u64 {
    SCROLLBACK_HEAD.load(AtomicOrdering::Relaxed)
}

/// Raw pointer to the scrollback ring's first byte. Use with
/// `(head + i) & (SCROLLBACK_SIZE - 1)` for the actual index. Read-only
/// via `read_volatile`; do not write through this pointer.
pub fn scrollback_buf_ptr() -> *const u8 {
    (&raw const SCROLLBACK_BUF) as *const u8
}

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

// ============================================================================
// Scrollback pager — let the user scroll back through console history
// ============================================================================
//
// Lines printed are recorded into the byte ring above (re-enabled in the
// console write path). `scroll_view` walks the ring to render a window of
// history; while scrolled (VIEW_OFFSET != 0) the live console suppresses
// drawing so the frozen view isn't overwritten (output still records).

/// Lines scrolled UP from the live bottom. 0 = live.
static VIEW_OFFSET: AtomicU64 = AtomicU64::new(0);

/// True while viewing history — the console write path checks this and skips
/// drawing (but still records) so the frozen view stays put.
pub fn is_scrolled() -> bool {
    VIEW_OFFSET.load(AtomicOrdering::Relaxed) != 0
}

/// Index just AFTER the `n`th newline walking back from `from`, i.e. the start
/// of the line `n` lines above `from`. Clamped to the oldest recorded byte.
fn index_back_lines(head: u64, from: u64, n: usize) -> u64 {
    let recorded = (head as usize).min(SCROLLBACK_SIZE) as u64;
    let oldest = head.wrapping_sub(recorded);
    let buf_ptr = (&raw const SCROLLBACK_BUF) as *const u8;
    let mut idx = from;
    let mut seen = 0usize;
    while idx > oldest {
        idx = idx.wrapping_sub(1);
        let pos = (idx as usize) & (SCROLLBACK_SIZE - 1);
        let byte = unsafe { core::ptr::read_volatile(buf_ptr.add(pos)) };
        if byte == b'\n' {
            seen += 1;
            if seen > n {
                return idx.wrapping_add(1); // start of the line below this NL
            }
        }
    }
    oldest
}

/// Total recorded lines (newline count) — the scroll-up limit.
fn recorded_lines(head: u64) -> usize {
    let recorded = (head as usize).min(SCROLLBACK_SIZE);
    let oldest = head.wrapping_sub(recorded as u64);
    let buf_ptr = (&raw const SCROLLBACK_BUF) as *const u8;
    let mut idx = oldest;
    let mut nl = 0usize;
    while idx != head {
        let pos = (idx as usize) & (SCROLLBACK_SIZE - 1);
        if unsafe { core::ptr::read_volatile(buf_ptr.add(pos)) } == b'\n' {
            nl += 1;
        }
        idx = idx.wrapping_add(1);
    }
    nl
}

/// Render the ring window for `offset` lines above the live bottom: `rows`
/// lines, cleared and replayed via put_byte. At offset 0 this replays right up
/// to HEAD, so the live cursor ends correctly and live drawing can resume.
fn render_from_offset(offset: u64) {
    let head = SCROLLBACK_HEAD.load(AtomicOrdering::Relaxed);
    let rows = match *CONSOLE.lock() {
        Some(ref c) => c.rows,
        None => return,
    };
    let end = index_back_lines(head, head, offset as usize);
    let start = index_back_lines(head, end, rows.saturating_sub(1));
    if let Some(ref mut c) = *CONSOLE.lock() {
        c.clear_all();
        c.cursor_x = 0;
        c.cursor_y = 0;
        let buf_ptr = (&raw const SCROLLBACK_BUF) as *const u8;
        let mut idx = start;
        while idx != end {
            // Don't let a trailing newline at the window edge scroll us.
            let pos = (idx as usize) & (SCROLLBACK_SIZE - 1);
            let byte = unsafe { core::ptr::read_volatile(buf_ptr.add(pos)) };
            if byte == b'\n' && idx.wrapping_add(1) == end {
                break;
            }
            c.put_byte(byte);
            idx = idx.wrapping_add(1);
        }
    }
}

/// Scroll the console view by `lines` (positive = older/up, negative =
/// newer/down). Clamps to [live, oldest]; re-renders the window. Reaching 0
/// returns to live (subsequent output draws normally again).
pub fn scroll_view(lines: isize) {
    let head = SCROLLBACK_HEAD.load(AtomicOrdering::Relaxed);
    let max_back = recorded_lines(head) as isize;
    let cur = VIEW_OFFSET.load(AtomicOrdering::Relaxed) as isize;
    let next = (cur + lines).clamp(0, max_back);
    VIEW_OFFSET.store(next as u64, AtomicOrdering::Relaxed);
    render_from_offset(next as u64);
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
        cols: width / CELL_W,
        rows: height / CELL_H,
    };

    // Publish the M6 drawing surface (independent view over the same bytes).
    *SURFACE.lock() = Some(FbSurface {
        addr: fb_addr,
        len: fb_len,
        width,
        height,
        stride,
        bpp: bytes_per_pixel,
        format,
    });

    // Clear screen to background.
    console.clear_all();
    *CONSOLE.lock() = Some(console);
}

/// Write a string to the framebuffer console (no-op if not initialized).
pub fn write_str(s: &str) {
    for byte in s.bytes() {
        scrollback_push(byte);
    }
    // While the user is viewing history, record but don't draw (freeze view).
    if is_scrolled() {
        return;
    }
    if let Some(ref mut c) = *CONSOLE.lock() {
        for byte in s.bytes() {
            c.put_byte(byte);
        }
    }
}

/// Clear the framebuffer console to the background and home the cursor. Used
/// when a full-screen overlay (the agent TUI) tears down, so the next output
/// resumes from a clean top-left instead of underneath the leftover panes.
pub fn clear() {
    if let Some(ref mut c) = *CONSOLE.lock() {
        c.clear_all();
        c.cursor_x = 0;
        c.cursor_y = 0;
    }
}

/// `core::fmt` adapter so `format_args!` can target the framebuffer.
struct ConsoleWriter;

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Record into the lock-free byte ring (scrollback pager + fault dump).
        // The earlier instability was a Mutex<[u8; 64K]>; this ring is lock-free
        // (atomic head + volatile writes), which is why it's safe to re-enable.
        for byte in s.bytes() {
            scrollback_push(byte);
        }
        // Freeze drawing while the user is scrolled back into history.
        if is_scrolled() {
            return Ok(());
        }
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
        let row_pixels = CELL_H;
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
        let base_x = col * CELL_W;
        let base_y = row * CELL_H;
        // Clear the whole cell first (incl. the inter-row gap) so scaled glyphs
        // and the gap band are clean even when overwriting prior text.
        for cy in 0..CELL_H {
            for cx in 0..CELL_W {
                self.put_pixel(base_x + cx, base_y + cy, BG);
            }
        }
        for (gy, row_bits) in glyph.iter().enumerate() {
            for gx in 0..FONT_WIDTH {
                // Bit 7 = leftmost pixel.
                if (row_bits >> (7 - gx)) & 1 == 0 {
                    continue;
                }
                // Paint a FONT_SCALE x FONT_SCALE block per source pixel.
                for sy in 0..FONT_SCALE {
                    for sx in 0..FONT_SCALE {
                        self.put_pixel(
                            base_x + gx * FONT_SCALE + sx,
                            base_y + gy * FONT_SCALE + sy,
                            FG,
                        );
                    }
                }
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
