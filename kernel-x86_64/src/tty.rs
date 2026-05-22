//! M7/M8 TTY console — TrueType / anti-aliased text console over the M6 fb.
//!
//! This is the "wire M7/M8 into a console output path" piece: a cursor-managed
//! text console that renders glyphs through the real font stack instead of the
//! 8x8 bitmap. It owns a pixel *region* of the framebuffer, tracks a pen, wraps
//! at the right edge, scrolls the region up a line when the cursor falls off the
//! bottom, and renders in one of two modes:
//!
//!   - `Aa::Sharp`  — M7 1-bit scanline fill (`font::FaceCtx`, one parse per
//!                    `write`, drawn glyph-by-glyph so it can wrap mid-line).
//!   - `Aa::Smooth` — M8 tiny-skia anti-aliased text (`gfx2d::aa_draw_text`,
//!                    one pixmap per line; no mid-line wrap, so keep AA lines
//!                    inside the region width).
//!
//! It is deliberately NOT the kernel's default `print!` sink: the boot console
//! stays on the fast bitmap font (serial is the source of truth, and the M7
//! glyph rasterizer's ~16 KiB stack frame must not run on the interrupt/syscall
//! print path — see the #41/#55 stack-sensitivity notes). DEMO 39 drives this
//! console and verifies it headlessly via pixel readback.

use crate::framebuffer::{self as fb, Color};
use crate::{font, gfx2d};

/// Glyph rendering mode for a `write`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Aa {
    /// M7: crisp 1-bit scanline fill.
    Sharp,
    /// M8: tiny-skia anti-aliased fill.
    Smooth,
}

/// A TrueType text console bound to a framebuffer region. Cursor state is in
/// pixels; `line_top` is the top of the current text line, `pen_x` the next
/// glyph's left edge.
pub struct TtyConsole {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    px: f32,
    line_h: usize,
    fg: Color,
    bg: Color,
    pen_x: usize,
    line_top: usize,
}

impl TtyConsole {
    /// Create a console over region `(x0, y0, w, h)` with em height `px`,
    /// foreground `fg` on background `bg`. Clears the region to `bg`.
    pub fn new(x0: usize, y0: usize, w: usize, h: usize, px: f32, fg: Color, bg: Color) -> Self {
        let line_h = font::line_height(px).max(px as usize + 2);
        fb::fb_fill_rect(x0, y0, w, h, bg);
        Self { x0, y0, w, h, px, line_h, fg, bg, pen_x: x0, line_top: y0 }
    }

    /// Current line's baseline (≈80% down the line box — leaves room for
    /// descenders). Matches what `draw_char` / `aa_draw_text` expect.
    #[inline]
    fn baseline(&self) -> usize {
        self.line_top + (self.line_h * 4) / 5
    }

    /// Advance to the start of the next line, scrolling the region up by one
    /// line if the next line wouldn't fit.
    fn newline(&mut self) {
        self.pen_x = self.x0;
        if self.line_top + 2 * self.line_h <= self.y0 + self.h {
            self.line_top += self.line_h;
        } else {
            fb::fb_scroll_region(self.x0, self.y0, self.w, self.h, self.line_h, self.bg);
            // line_top stays pinned to the last line.
        }
    }

    /// Pixel rows occupied (current baseline relative to the region top) —
    /// used by tests to confirm the cursor advanced/scrolled.
    pub fn cursor_baseline(&self) -> usize {
        self.baseline()
    }

    /// Write `text` in `mode`, handling `\n`, right-edge wrap (Sharp), and
    /// bottom scroll.
    pub fn write(&mut self, mode: Aa, text: &str) {
        match mode {
            Aa::Sharp => {
                // One parse for the whole write; draw glyph-by-glyph so we can
                // wrap at the region's right edge.
                font::with_face(self.px, |face| {
                    for ch in text.chars() {
                        if ch == '\n' {
                            self.newline();
                            continue;
                        }
                        let adv = face.advance(ch);
                        if self.pen_x as f32 + adv > (self.x0 + self.w) as f32 {
                            self.newline();
                        }
                        let nx = face.draw_char(self.pen_x as f32, self.baseline() as f32, ch, self.fg);
                        self.pen_x += nx as usize;
                    }
                });
            }
            Aa::Smooth => {
                // tiny-skia rasterizes a whole run per pixmap, so render one
                // line segment at a time (no mid-line wrap in this mode).
                let mut first = true;
                for seg in text.split('\n') {
                    if !first {
                        self.newline();
                    }
                    first = false;
                    if !seg.is_empty() {
                        let end = gfx2d::aa_draw_text(self.pen_x, self.baseline(), seg, self.px, self.fg);
                        self.pen_x = end;
                    }
                }
            }
        }
    }
}
