//! M8 — anti-aliased 2D vector rendering via tiny-skia.
//!
//! tiny-skia (no_std + the kernel global allocator) rasterizes paths with
//! real anti-aliasing into an in-heap `Pixmap`; we then blit the pixmap to
//! the M6 framebuffer with `fb_blit`. This is the AA story M7's 1-bit glyph
//! fill deferred: stroked Béziers, filled shapes, sub-pixel coverage.
//!
//! `fill_path` / `stroke_path` are thin wrappers a future drawing API can
//! grow from; `aa_scene` draws a self-contained test scene used by DEMO 38.

use alloc::vec::Vec;
use tiny_skia::{
    Color as SkColor, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform,
};
use ttf_parser::{Face, OutlineBuilder};
use crate::framebuffer::{self as fb, Color};

static FONT_DATA: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");

/// Blit a finished pixmap to the framebuffer at `(ox, oy)`, converting
/// premultiplied RGBA → the framebuffer's packed color. Returns
/// `(non_background_px, antialiased_edge_px)`: the second count is pixels
/// that are neither pure background (black) nor a pure source color — i.e.
/// blended AA-edge pixels, the headless signature that anti-aliasing ran.
fn blit_and_measure(pm: &Pixmap, ox: usize, oy: usize, w: usize, h: usize) -> (usize, usize) {
    let mut buf: Vec<Color> = Vec::with_capacity(w * h);
    let mut lit = 0usize;
    let mut aa = 0usize;
    for p in pm.pixels() {
        let (r, g, b) = (p.red(), p.green(), p.blue());
        buf.push(fb::rgb(r, g, b));
        let is_bg = r == 0 && g == 0 && b == 0;
        if !is_bg {
            lit += 1;
            // Pure source colors we draw with: white and green. Anything
            // else non-black is a blended (anti-aliased) edge pixel.
            let pure_white = r == 255 && g == 255 && b == 255;
            let pure_green = r == 0 && g == 255 && b == 0;
            if !pure_white && !pure_green {
                aa += 1;
            }
        }
    }
    fb::fb_blit(&buf, ox, oy, w, h);
    (lit, aa)
}

/// Render a self-contained anti-aliased scene into a `w×h` pixmap and blit it
/// to the framebuffer at `(ox, oy)`. Returns `(lit, aa_edge)` pixel counts
/// (measured on the pixmap, independent of the framebuffer/console).
pub fn aa_scene(ox: usize, oy: usize, w: usize, h: usize) -> (usize, usize) {
    let mut pm = match Pixmap::new(w as u32, h as u32) {
        Some(p) => p,
        None => return (0, 0),
    };
    pm.fill(SkColor::from_rgba8(0, 0, 0, 255)); // opaque black background

    let wf = w as f32;
    let hf = h as f32;

    // 1) Filled, anti-aliased circle (white).
    {
        let mut pb = PathBuilder::new();
        pb.push_circle(wf * 0.30, hf * 0.50, hf * 0.32);
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(SkColor::from_rgba8(255, 255, 255, 255));
            paint.anti_alias = true;
            pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    // 2) Stroked, anti-aliased cubic Bézier (green).
    {
        let mut pb = PathBuilder::new();
        pb.move_to(wf * 0.52, hf * 0.85);
        pb.cubic_to(wf * 0.62, hf * 0.05, wf * 0.92, hf * 0.95, wf * 0.97, hf * 0.20);
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(SkColor::from_rgba8(0, 255, 0, 255));
            paint.anti_alias = true;
            let mut stroke = Stroke::default();
            stroke.width = 3.0;
            pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    blit_and_measure(&pm, ox, oy, w, h)
}

// ============================================================================
// M8 — anti-aliased TTF text (the AA counterpart to M7's 1-bit fill)
// ============================================================================
//
// Feeds each glyph's outline into a tiny_skia `PathBuilder` (one path for the
// whole run), fills it with `anti_alias = true` into a run-sized pixmap, and
// blits to the framebuffer. The pixmap background is opaque black, matching a
// console region that's cleared to black — so the blit paints text + its own
// dark cell, no compositing needed. This is what the TTY console's "Smooth"
// mode renders through.

/// Bridges `ttf_parser` glyph outlines into a tiny_skia path, transforming
/// font units (y-up) into pixmap pixel space (y-down) via `scale`, the pen
/// origin `ox`, and the baseline `oy`.
struct SkOutline<'a> {
    pb: &'a mut PathBuilder,
    scale: f32,
    ox: f32,
    oy: f32,
}

impl SkOutline<'_> {
    #[inline]
    fn p(&self, fx: f32, fy: f32) -> (f32, f32) {
        (self.ox + fx * self.scale, self.oy - fy * self.scale)
    }
}

impl OutlineBuilder for SkOutline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (a, b) = self.p(x, y);
        self.pb.move_to(a, b);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (a, b) = self.p(x, y);
        self.pb.line_to(a, b);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, b) = self.p(x1, y1);
        let (c, d) = self.p(x, y);
        self.pb.quad_to(a, b, c, d);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b) = self.p(x1, y1);
        let (c, d) = self.p(x2, y2);
        let (e, f) = self.p(x, y);
        self.pb.cubic_to(a, b, c, d, e, f);
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

/// Anti-aliased text: draw `text` with its baseline at `(x, baseline_y)`,
/// glyphs `px` tall, in `color`, via tiny_skia AA. Returns the end pen x.
/// The run is rasterized into one pixmap (opaque-black background) and blitted.
pub fn aa_draw_text(x: usize, baseline_y: usize, text: &str, px: f32, color: Color) -> usize {
    let face = match Face::parse(FONT_DATA, 0) {
        Ok(f) => f,
        Err(_) => return x,
    };
    let upem = face.units_per_em() as f32;
    if upem <= 0.0 {
        return x;
    }
    let scale = px / upem;
    let ascent = face.ascender() as f32 * scale;
    let descent = -(face.descender() as f32) * scale; // descender() is negative
    let pad = 2.0f32;

    // Total run width (sum of advances) sizes the pixmap.
    let mut total = 0.0f32;
    for ch in text.chars() {
        if let Some(g) = face.glyph_index(ch) {
            total += face.glyph_hor_advance(g).unwrap_or(0) as f32 * scale;
        }
    }

    let pm_w = (total + pad * 2.0) as u32 + 1;
    let pm_h = (ascent + descent + pad * 2.0) as u32 + 1;
    let mut pm = match Pixmap::new(pm_w, pm_h) {
        Some(p) => p,
        None => return x,
    };
    pm.fill(SkColor::from_rgba8(0, 0, 0, 255));

    // Build one path covering every glyph, advancing the pen across the run.
    let baseline_pm = ascent + pad;
    let mut pb = PathBuilder::new();
    let mut pen = pad;
    for ch in text.chars() {
        if let Some(g) = face.glyph_index(ch) {
            let mut o = SkOutline { pb: &mut pb, scale, ox: pen, oy: baseline_pm };
            let _ = face.outline_glyph(g, &mut o);
            pen += face.glyph_hor_advance(g).unwrap_or(0) as f32 * scale;
        }
    }
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = (color & 0xFF) as u8;
        paint.set_color(SkColor::from_rgba8(r, g, b, 255));
        paint.anti_alias = true;
        pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }

    // Blit the run; its top-left lands so the baseline sits at baseline_y.
    let oy = (baseline_y as f32 - baseline_pm).max(0.0) as usize;
    let w = pm.width() as usize;
    let h = pm.height() as usize;
    let mut buf: Vec<Color> = Vec::with_capacity(w * h);
    for p in pm.pixels() {
        buf.push(fb::rgb(p.red(), p.green(), p.blue()));
    }
    fb::fb_blit(&buf, x, oy, w, h);
    (x as f32 + total) as usize
}
