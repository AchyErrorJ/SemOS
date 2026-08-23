//! M7 — TrueType font rasterization over the M6 framebuffer.
//!
//! Real glyph rendering, not the 8x16 bitmap console. We parse an embedded
//! TTF with `ttf-parser` (zero-allocation, no_std), flatten each glyph's
//! outline (lines + quadratic/cubic Béziers) into pixel-space edges in a
//! fixed stack buffer (no heap — the kernel has no allocator), then
//! scanline-fill them (even-odd rule, 1-bit coverage; anti-aliasing is the
//! M8 tiny-skia job) straight into the framebuffer via `fb_fill_rect`.
//!
//! `fb_draw_text(x, baseline_y, s, px, color)` lays glyphs left-to-right at
//! the requested pixel height. Latin/ASCII only for now (no shaping/kerning).
//!
//! Font: Noto Sans Regular (SIL Open Font License 1.1 — see
//! `kernel-x86_64/assets/OFL.txt`).

use ttf_parser::{Face, OutlineBuilder};
use crate::framebuffer::{self as fb, Color};

static FONT_DATA: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");

/// Max line-segments accumulated per glyph. ASCII glyphs flatten to well
/// under this; complex CJK would need more (out of scope).
const MAX_EDGES: usize = 1024;
/// Max edge crossings on a single scanline.
const MAX_XS: usize = 64;
/// Segments per Bézier when flattening (fixed-step — fine at console sizes).
const BEZIER_STEPS: usize = 8;

/// One pixel-space edge of a glyph outline.
#[derive(Clone, Copy)]
struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Edge {
    const EMPTY: Edge = Edge { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 };
}

/// Collects a single glyph's outline as pixel-space edges. Implements
/// `ttf_parser::OutlineBuilder`; coordinates arrive in font units (y-up) and
/// are transformed to pixel space (y-down) here via `scale` and the pen
/// origin `(ox, oy)` (the glyph's left edge on the baseline).
struct GlyphRaster {
    edges: [Edge; MAX_EDGES],
    n: usize,
    scale: f32,
    ox: f32,
    oy: f32,
    // Current pen + contour-start, in font units (pre-transform).
    cx: f32,
    cy: f32,
    sx: f32,
    sy: f32,
    overflow: bool,
}

impl GlyphRaster {
    fn new(scale: f32, ox: f32, oy: f32) -> Self {
        Self {
            edges: [Edge::EMPTY; MAX_EDGES],
            n: 0,
            scale,
            ox,
            oy,
            cx: 0.0,
            cy: 0.0,
            sx: 0.0,
            sy: 0.0,
            overflow: false,
        }
    }

    /// Transform a font-unit point to pixel space (y flipped, scaled, offset).
    #[inline]
    fn px(&self, fx: f32, fy: f32) -> (f32, f32) {
        (self.ox + fx * self.scale, self.oy - fy * self.scale)
    }

    /// Push a pixel-space edge from font-unit endpoints.
    fn push_edge(&mut self, fx0: f32, fy0: f32, fx1: f32, fy1: f32) {
        if self.n >= MAX_EDGES {
            self.overflow = true;
            return;
        }
        let (x0, y0) = self.px(fx0, fy0);
        let (x1, y1) = self.px(fx1, fy1);
        self.edges[self.n] = Edge { x0, y0, x1, y1 };
        self.n += 1;
    }
}

impl OutlineBuilder for GlyphRaster {
    fn move_to(&mut self, x: f32, y: f32) {
        self.cx = x;
        self.cy = y;
        self.sx = x;
        self.sy = y;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push_edge(self.cx, self.cy, x, y);
        self.cx = x;
        self.cy = y;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        // Quadratic Bézier B(t) = (1-t)^2 P0 + 2(1-t)t P1 + t^2 P2.
        let (p0x, p0y) = (self.cx, self.cy);
        let mut prevx = p0x;
        let mut prevy = p0y;
        for i in 1..=BEZIER_STEPS {
            let t = i as f32 / BEZIER_STEPS as f32;
            let mt = 1.0 - t;
            let bx = mt * mt * p0x + 2.0 * mt * t * x1 + t * t * x;
            let by = mt * mt * p0y + 2.0 * mt * t * y1 + t * t * y;
            self.push_edge(prevx, prevy, bx, by);
            prevx = bx;
            prevy = by;
        }
        self.cx = x;
        self.cy = y;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        // Cubic Bézier.
        let (p0x, p0y) = (self.cx, self.cy);
        let mut prevx = p0x;
        let mut prevy = p0y;
        for i in 1..=BEZIER_STEPS {
            let t = i as f32 / BEZIER_STEPS as f32;
            let mt = 1.0 - t;
            let a = mt * mt * mt;
            let b = 3.0 * mt * mt * t;
            let c = 3.0 * mt * t * t;
            let d = t * t * t;
            let bx = a * p0x + b * x1 + c * x2 + d * x;
            let by = a * p0y + b * y1 + c * y2 + d * y;
            self.push_edge(prevx, prevy, bx, by);
            prevx = bx;
            prevy = by;
        }
        self.cx = x;
        self.cy = y;
    }

    fn close(&mut self) {
        // Close the contour back to its start.
        if self.cx != self.sx || self.cy != self.sy {
            self.push_edge(self.cx, self.cy, self.sx, self.sy);
        }
        self.cx = self.sx;
        self.cy = self.sy;
    }
}

/// Clip rect `(x0, y0, x1, y1)` — half-open pixel bounds a draw may touch.
/// [`full_clip`] is the whole framebuffer (the historical behaviour).
pub type Clip = (usize, usize, usize, usize);

/// The whole framebuffer as a clip rect — the default for callers that never
/// heard of panes (editor, legacy demos, boot console).
pub fn full_clip() -> Clip {
    let (w, h) = fb::fb_dimensions();
    (0, 0, w, h)
}

/// Scanline-fill the collected edges into the framebuffer (even-odd rule,
/// 1-bit coverage). Clips to `clip` (intersected with the framebuffer bounds),
/// so pane-owned text can never spill ink into a neighbouring pane.
fn fill_glyph(r: &GlyphRaster, color: Color, clip: Clip) {
    if r.n == 0 {
        return;
    }
    let (fb_w, fb_h) = fb::fb_dimensions();
    let (cx0, cy0, cx1, cy1) = clip;
    let cx1 = cx1.min(fb_w);
    let cy1 = cy1.min(fb_h);
    if cx0 >= cx1 || cy0 >= cy1 {
        return;
    }

    // Vertical extent of the glyph in pixel space.
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for e in &r.edges[..r.n] {
        let (lo, hi) = if e.y0 < e.y1 { (e.y0, e.y1) } else { (e.y1, e.y0) };
        if lo < min_y { min_y = lo; }
        if hi > max_y { max_y = hi; }
    }
    let mut row = if min_y < 0.0 { 0 } else { min_y as usize };
    let row_end = if max_y < 0.0 { 0 } else { (max_y as usize + 1).min(fb_h) };
    // Intersect with the clip's vertical span.
    let mut row = row.max(cy0);
    let row_end = row_end.min(cy1);

    while row < row_end {
        let sy = row as f32 + 0.5; // sample at pixel-row center
        // Collect x-intersections of edges crossing this scanline.
        let mut xs = [0.0f32; MAX_XS];
        let mut nx = 0usize;
        for e in &r.edges[..r.n] {
            let (ya, xa, yb, xb) = if e.y0 <= e.y1 {
                (e.y0, e.x0, e.y1, e.x1)
            } else {
                (e.y1, e.x1, e.y0, e.x0)
            };
            // Half-open [ya, yb) so shared vertices aren't counted twice.
            if sy >= ya && sy < yb && yb > ya {
                let t = (sy - ya) / (yb - ya);
                let xc = xa + t * (xb - xa);
                if nx < MAX_XS {
                    xs[nx] = xc;
                    nx += 1;
                }
            }
        }
        // Insertion-sort the crossings (small n, no alloc).
        let mut i = 1;
        while i < nx {
            let v = xs[i];
            let mut j = i;
            while j > 0 && xs[j - 1] > v {
                xs[j] = xs[j - 1];
                j -= 1;
            }
            xs[j] = v;
            i += 1;
        }
        // Even-odd: fill spans between successive crossing pairs.
        let mut k = 0;
        while k + 1 < nx {
            let xl = xs[k];
            let xr = xs[k + 1];
            let mut x0 = if xl < 0.0 { 0 } else { xl as usize };
            // Manual ceil (f32::ceil is std-only; we're no_std).
            let x1 = if xr < 0.0 {
                0
            } else {
                let xi = xr as usize;
                (if xr > xi as f32 { xi + 1 } else { xi }).min(fb_w)
            };
            // Intersect the span with the clip's horizontal span — this is
            // the line that keeps pane text inside its pane. (x0 >= fb_w is
            // now impossible when x0 < x1: x1 <= cx1 <= fb_w.)
            let x0 = x0.max(cx0);
            let x1 = x1.min(cx1);
            if x0 < x1 {
                fb::fb_fill_rect(x0, row, x1 - x0, 1, color);
            }
            k += 2;
        }
        row += 1;
    }
}

/// Draw `text` with its baseline at `(x, baseline_y)`, glyphs `px` pixels
/// tall (em height), in `color`. Returns the pen x-advance (end x).
/// Latin/ASCII; unknown chars are skipped (advance only if they have one).
pub fn fb_draw_text(x: usize, baseline_y: usize, text: &str, px: f32, color: Color) -> usize {
    let face = match Face::parse(FONT_DATA, 0) {
        Ok(f) => f,
        Err(_) => return x,
    };
    let upem = face.units_per_em() as f32;
    if upem <= 0.0 {
        return x;
    }
    let scale = px / upem;
    let mut pen_x = x as f32;
    let baseline = baseline_y as f32;

    for ch in text.chars() {
        let gid = match face.glyph_index(ch) {
            Some(g) => g,
            None => continue,
        };
        // Rasterize the glyph at the current pen origin.
        let mut r = GlyphRaster::new(scale, pen_x, baseline);
        let _ = face.outline_glyph(gid, &mut r);
        fill_glyph(&r, color, full_clip());
        // Advance the pen by the glyph's horizontal advance.
        if let Some(adv) = face.glyph_hor_advance(gid) {
            pen_x += adv as f32 * scale;
        }
    }
    pen_x as usize
}

/// Convenience: the font's recommended line height in pixels for size `px`.
pub fn line_height(px: f32) -> usize {
    let face = match Face::parse(FONT_DATA, 0) {
        Ok(f) => f,
        Err(_) => return px as usize,
    };
    let upem = face.units_per_em() as f32;
    let h = face.height() as f32;
    (h / upem * px) as usize
}

// ============================================================================
// Cached-face API for the TTY console (M7 path)
// ============================================================================
//
// `fb_draw_text` re-parses the whole TTF on every call — fine for the handful
// of strings DEMO 37 draws, but a per-glyph console wants to parse once and
// then render many glyphs. `with_face` parses the embedded face a single time
// and hands a `FaceCtx` to a closure; the console uses it to lay out and draw
// characters one at a time (so it can wrap and scroll), all on one parse.

/// A parsed face plus the px→font-unit scale, exposing the per-glyph
/// primitives the TTY console needs. Lives only for the `with_face` closure.
pub struct FaceCtx<'a> {
    face: Face<'a>,
    scale: f32,
    upem: f32,
    clip: Clip,
}

impl FaceCtx<'_> {
    /// Horizontal advance of `ch` in pixels (0 if the font has no such glyph).
    pub fn advance(&self, ch: char) -> f32 {
        match self.face.glyph_index(ch) {
            Some(gid) => self
                .face
                .glyph_hor_advance(gid)
                .map(|a| a as f32 * self.scale)
                .unwrap_or(0.0),
            None => 0.0,
        }
    }

    /// Draw `ch` with its left edge at `pen_x` on `baseline` (both in pixels),
    /// using M7's 1-bit scanline fill. Ink is clipped to this context's clip
    /// rect (full framebuffer unless built via [`with_face_clip`]). Returns
    /// the pen advance in pixels.
    pub fn draw_char(&self, pen_x: f32, baseline: f32, ch: char, color: Color) -> f32 {
        let gid = match self.face.glyph_index(ch) {
            Some(g) => g,
            None => return 0.0,
        };
        let mut r = GlyphRaster::new(self.scale, pen_x, baseline);
        let _ = self.face.outline_glyph(gid, &mut r);
        fill_glyph(&r, color, self.clip);
        self.face
            .glyph_hor_advance(gid)
            .map(|a| a as f32 * self.scale)
            .unwrap_or(0.0)
    }

    /// Recommended line height in pixels (font ascent+descent+gap scaled).
    pub fn line_height(&self) -> f32 {
        self.face.height() as f32 * self.scale
    }
}

/// Parse the embedded face once and run `f` with a `FaceCtx` for size `px`
/// (em height). Returns `None` (closure not run) if the font fails to parse.
/// The context clips to the full framebuffer; use [`with_face_clip`] to draw
/// inside a pane rect.
pub fn with_face<R>(px: f32, f: impl FnOnce(&FaceCtx) -> R) -> Option<R> {
    with_face_clip(px, full_clip(), f)
}

/// [`with_face`] with an explicit clip rect: every glyph drawn through the
/// context is clipped to `clip` (intersected with the framebuffer bounds).
pub fn with_face_clip<R>(px: f32, clip: Clip, f: impl FnOnce(&FaceCtx) -> R) -> Option<R> {
    let face = Face::parse(FONT_DATA, 0).ok()?;
    let upem = face.units_per_em() as f32;
    if upem <= 0.0 {
        return None;
    }
    let ctx = FaceCtx { face, scale: px / upem, upem, clip };
    Some(f(&ctx))
}
