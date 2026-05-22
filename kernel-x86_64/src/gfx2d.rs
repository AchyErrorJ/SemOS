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
use crate::framebuffer::{self as fb, Color};

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
