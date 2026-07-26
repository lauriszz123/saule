//! Software rasterizer: pixel surfaces, blend modes, an antialiased polygon
//! filler, and the two blitters (alpha masks for glyphs, ARGB surfaces for
//! canvases).
//!
//! Everything here works in **device coordinates** — [`crate::state`] applies
//! the current transform before calling in. Coverage is computed by sampling
//! [`SUBSAMPLES`] sub-scanlines per pixel row and accumulating exact horizontal
//! span overlap, which is what gives shape edges and text their smooth edges
//! without a GPU.

use crate::geom::{Point, Transform};

/// Sub-scanlines sampled per pixel row. Four is the usual quality/speed
/// sweet spot: vertical edges are exact either way, and near-horizontal ones
/// resolve to 5 distinct coverage levels.
const SUBSAMPLES: usize = 4;

// ---------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------

/// An ARGB8888 pixel buffer.
///
/// The window's framebuffer ignores the alpha byte (minifb only reads `0RGB`),
/// but canvases keep it meaningful so a canvas can be composited back over the
/// screen with real transparency.
pub struct Surface {
    pub buf: Vec<u32>,
    pub w: usize,
    pub h: usize,
}

impl Surface {
    /// A fully transparent surface.
    pub fn new(w: usize, h: usize) -> Self {
        Surface {
            buf: vec![0; w * h],
            w,
            h,
        }
    }

    /// An opaque black surface — the window's initial framebuffer.
    pub fn opaque(w: usize, h: usize) -> Self {
        Surface {
            buf: vec![0xFF00_0000; w * h],
            w,
            h,
        }
    }

    /// Overwrite every pixel inside `clip` with a straight colour, ignoring the
    /// blend mode. This is `Graphics.clear`.
    pub fn clear(&mut self, color: [f32; 4], clip: Rect) {
        let px = pack(color[3], color[0], color[1], color[2]);
        let (x0, y0, x1, y1) = clip.pixel_bounds(self.w, self.h);
        if x0 == 0 && y0 == 0 && x1 == self.w && y1 == self.h {
            self.buf.fill(px);
            return;
        }
        for y in y0..y1 {
            self.buf[y * self.w + x0..y * self.w + x1].fill(px);
        }
    }

    /// Composite one source sample onto pixel `idx`.
    ///
    /// `alpha` is the source colour's own alpha and `coverage` is the
    /// rasterizer's antialiasing weight; they are kept separate because
    /// [`BlendMode::Replace`] must overwrite the destination at full coverage
    /// while still feathering its edges.
    #[inline]
    fn blend(&mut self, idx: usize, color: [f32; 4], coverage: f32, mode: BlendMode) {
        let cov = coverage.clamp(0.0, 1.0);
        if cov <= 0.0 {
            return;
        }
        let (sr, sg, sb, ca) = (color[0], color[1], color[2], color[3]);
        let sa = ca * cov;
        if sa <= 0.0 && mode != BlendMode::Replace {
            return;
        }

        let dst = self.buf[idx];
        let (da, dr, dg, db) = unpack(dst);

        let (r, g, b, a) = match mode {
            BlendMode::Alpha => (
                sr * sa + dr * (1.0 - sa),
                sg * sa + dg * (1.0 - sa),
                sb * sa + db * (1.0 - sa),
                sa + da * (1.0 - sa),
            ),
            BlendMode::Add => (dr + sr * sa, dg + sg * sa, db + sb * sa, da),
            BlendMode::Subtract => (dr - sr * sa, dg - sg * sa, db - sb * sa, da),
            // Modulate the destination toward `src * dst`, fading the effect
            // out with alpha so a translucent multiply is a partial darkening.
            BlendMode::Multiply => (
                dr * (sr * sa + 1.0 - sa),
                dg * (sg * sa + 1.0 - sa),
                db * (sb * sa + 1.0 - sa),
                da * (ca * sa + 1.0 - sa),
            ),
            BlendMode::Screen => (
                sr * sa + dr - sr * sa * dr,
                sg * sa + dg - sg * sa * dg,
                sb * sa + db - sb * sa * db,
                sa + da * (1.0 - sa),
            ),
            // No blending at all: coverage just feathers the overwrite.
            BlendMode::Replace => (
                dr + (sr - dr) * cov,
                dg + (sg - dg) * cov,
                db + (sb - db) * cov,
                da + (ca - da) * cov,
            ),
        };
        self.buf[idx] = pack(a, r, g, b);
    }

    /// Sample a pixel, returning transparent black outside the surface.
    #[inline]
    fn sample_nearest(&self, x: f64, y: f64) -> (f32, f32, f32, f32) {
        let (xi, yi) = (x.floor() as i64, y.floor() as i64);
        if xi < 0 || yi < 0 || xi as usize >= self.w || yi as usize >= self.h {
            return (0.0, 0.0, 0.0, 0.0);
        }
        unpack(self.buf[yi as usize * self.w + xi as usize])
    }

    /// Bilinear sample at pixel-centre convention.
    #[inline]
    fn sample_linear(&self, x: f64, y: f64) -> (f32, f32, f32, f32) {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);

        let p00 = self.sample_nearest(x0 + 0.5, y0 + 0.5);
        let p10 = self.sample_nearest(x0 + 1.5, y0 + 0.5);
        let p01 = self.sample_nearest(x0 + 0.5, y0 + 1.5);
        let p11 = self.sample_nearest(x0 + 1.5, y0 + 1.5);

        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let mix = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32), t: f32| {
            (
                lerp(a.0, b.0, t),
                lerp(a.1, b.1, t),
                lerp(a.2, b.2, t),
                lerp(a.3, b.3, t),
            )
        };
        mix(mix(p00, p10, tx), mix(p01, p11, tx), ty)
    }
}

#[inline]
fn unpack(p: u32) -> (f32, f32, f32, f32) {
    const INV: f32 = 1.0 / 255.0;
    (
        ((p >> 24) & 0xFF) as f32 * INV,
        ((p >> 16) & 0xFF) as f32 * INV,
        ((p >> 8) & 0xFF) as f32 * INV,
        (p & 0xFF) as f32 * INV,
    )
}

#[inline]
fn pack(a: f32, r: f32, g: f32, b: f32) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (q(a) << 24) | (q(r) << 16) | (q(g) << 8) | q(b)
}

// ---------------------------------------------------------------------------
// Blend modes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    Alpha,
    Add,
    Subtract,
    Multiply,
    Screen,
    Replace,
}

impl BlendMode {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "alpha" => Ok(BlendMode::Alpha),
            "add" => Ok(BlendMode::Add),
            "subtract" => Ok(BlendMode::Subtract),
            "multiply" => Ok(BlendMode::Multiply),
            "screen" => Ok(BlendMode::Screen),
            "replace" | "none" => Ok(BlendMode::Replace),
            other => Err(format!(
                "unknown blend mode `{other}` (expected \"alpha\", \"add\", \
                 \"subtract\", \"multiply\", \"screen\", or \"replace\")"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BlendMode::Alpha => "alpha",
            BlendMode::Add => "add",
            BlendMode::Subtract => "subtract",
            BlendMode::Multiply => "multiply",
            BlendMode::Screen => "screen",
            BlendMode::Replace => "replace",
        }
    }
}

// ---------------------------------------------------------------------------
// Clip rectangles
// ---------------------------------------------------------------------------

/// A half-open device-space rectangle used for scissor clipping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
        }
    }

    pub fn surface(w: usize, h: usize) -> Self {
        Rect {
            x0: 0.0,
            y0: 0.0,
            x1: w as f64,
            y1: h as f64,
        }
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    /// The pixel index range this rectangle touches, clamped to a surface.
    fn pixel_bounds(&self, w: usize, h: usize) -> (usize, usize, usize, usize) {
        if self.is_empty() {
            return (0, 0, 0, 0);
        }
        let x0 = self.x0.floor().max(0.0) as usize;
        let y0 = self.y0.floor().max(0.0) as usize;
        let x1 = (self.x1.ceil().max(0.0) as usize).min(w);
        let y1 = (self.y1.ceil().max(0.0) as usize).min(h);
        if x1 <= x0 || y1 <= y0 {
            (0, 0, 0, 0)
        } else {
            (x0, y0, x1, y1)
        }
    }
}

// ---------------------------------------------------------------------------
// Paint — everything a draw call needs beyond its geometry
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Paint {
    /// Straight (non-premultiplied) RGBA in `0.0..=1.0`.
    pub color: [f32; 4],
    pub blend: BlendMode,
    /// Device-space clip, already intersected with the target's bounds.
    pub clip: Rect,
    /// When false, coverage is thresholded at 50% for hard, aliased edges
    /// (`Graphics.setLineStyle("rough")`).
    pub antialias: bool,
    /// Bilinear rather than nearest sampling for blits.
    pub linear_filter: bool,
}

impl Paint {
    #[inline]
    fn shape(&self, coverage: f32) -> f32 {
        if self.antialias {
            coverage
        } else if coverage >= 0.5 {
            1.0
        } else {
            0.0
        }
    }
}

// ---------------------------------------------------------------------------
// Polygon filling
// ---------------------------------------------------------------------------

/// One edge of the flattened polygon set, kept in "top to bottom" form with
/// its original direction recorded for the nonzero winding count.
struct Edge {
    x0: f64,
    y0: f64,
    dxdy: f64,
    y_top: f64,
    y_bot: f64,
    winding: i32,
}

/// Fill a set of polygons using the **nonzero winding rule**.
///
/// Passing several polygons at once is not just a convenience: overlapping
/// polygons that wind the same way union cleanly under this rule, which is how
/// [`crate::geom::stroke`] gets a thick polyline's segments and joins to merge
/// into one shape instead of double-blending at every corner.
pub fn fill_paths(surf: &mut Surface, paths: &[Vec<Point>], paint: &Paint) {
    let clip = paint.clip.intersect(&Rect::surface(surf.w, surf.h));
    if clip.is_empty() {
        return;
    }

    let mut edges: Vec<Edge> = Vec::new();
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);

    for path in paths {
        if path.len() < 3 {
            continue;
        }
        for i in 0..path.len() {
            let (ax, ay) = path[i];
            let (bx, by) = path[(i + 1) % path.len()];
            if !ax.is_finite() || !ay.is_finite() || !bx.is_finite() || !by.is_finite() {
                continue;
            }
            min_x = min_x.min(ax);
            max_x = max_x.max(ax);
            min_y = min_y.min(ay);
            max_y = max_y.max(ay);
            if ay == by {
                continue; // horizontal edges are never crossed by a scanline
            }
            edges.push(Edge {
                x0: ax,
                y0: ay,
                dxdy: (bx - ax) / (by - ay),
                y_top: ay.min(by),
                y_bot: ay.max(by),
                // Downward edges count +1, upward −1; the running sum is the
                // winding number that the nonzero rule tests against zero.
                winding: if ay < by { 1 } else { -1 },
            });
        }
    }
    if edges.is_empty() {
        return;
    }

    let bbox = Rect {
        x0: min_x,
        y0: min_y,
        x1: max_x,
        y1: max_y,
    };
    let (px0, py0, px1, py1) = bbox.intersect(&clip).pixel_bounds(surf.w, surf.h);
    if px1 <= px0 || py1 <= py0 {
        return;
    }

    // Sort by top edge so the active list can be advanced with a cursor rather
    // than rescanning every edge for every sub-scanline.
    edges.sort_by(|a, b| a.y_top.partial_cmp(&b.y_top).unwrap_or(std::cmp::Ordering::Equal));

    let width = px1 - px0;
    let mut coverage = vec![0.0f32; width];
    let mut crossings: Vec<(f64, i32)> = Vec::with_capacity(16);
    let mut active: Vec<usize> = Vec::with_capacity(16);
    let mut cursor = 0usize;

    let sub_weight = 1.0 / SUBSAMPLES as f32;

    for py in py0..py1 {
        coverage.fill(0.0);
        let row_bot = py as f64 + 1.0;

        // Admit edges that start within this row, retire ones that ended above.
        while cursor < edges.len() && edges[cursor].y_top < row_bot {
            active.push(cursor);
            cursor += 1;
        }
        active.retain(|&i| edges[i].y_bot > py as f64);

        for s in 0..SUBSAMPLES {
            let sy = py as f64 + (s as f64 + 0.5) / SUBSAMPLES as f64;
            crossings.clear();
            for &i in &active {
                let e = &edges[i];
                if sy >= e.y_top && sy < e.y_bot {
                    crossings.push((e.x0 + (sy - e.y0) * e.dxdy, e.winding));
                }
            }
            if crossings.len() < 2 {
                continue;
            }
            crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

            let mut winding = 0;
            let mut span_start = 0.0f64;
            for &(x, w) in crossings.iter() {
                let was_inside = winding != 0;
                winding += w;
                let is_inside = winding != 0;
                if !was_inside && is_inside {
                    span_start = x;
                } else if was_inside && !is_inside {
                    add_span(
                        &mut coverage,
                        px0,
                        px1,
                        span_start.max(clip.x0),
                        x.min(clip.x1),
                        sub_weight,
                    );
                }
            }
        }

        let row = py * surf.w;
        for (i, &cov) in coverage.iter().enumerate() {
            let c = paint.shape(cov);
            if c > 0.0 {
                surf.blend(row + px0 + i, paint.color, c, paint.blend);
            }
        }
    }
}

/// Accumulate a horizontal span's exact per-pixel overlap into `coverage`.
#[inline]
fn add_span(coverage: &mut [f32], px0: usize, px1: usize, x_start: f64, x_end: f64, weight: f32) {
    let lo = x_start.max(px0 as f64);
    let hi = x_end.min(px1 as f64);
    if hi <= lo {
        return;
    }
    let first = lo.floor() as usize;
    let last = (hi.ceil() as usize).min(px1);
    for px in first..last {
        let l = lo.max(px as f64);
        let r = hi.min(px as f64 + 1.0);
        if r > l {
            coverage[px - px0] += (r - l) as f32 * weight;
        }
    }
}

// ---------------------------------------------------------------------------
// Blitting
// ---------------------------------------------------------------------------

/// An 8-bit coverage bitmap — a rasterized glyph.
pub struct Mask {
    pub data: Vec<u8>,
    pub w: usize,
    pub h: usize,
}

/// Draw an alpha mask through `xform`, tinted with the paint colour.
///
/// `xform` maps mask pixel space (`0..w`, `0..h`) into device space. The common
/// case — an unrotated, unscaled glyph at an integer position — is detected and
/// copied directly, which keeps text crisp instead of resampling it into a
/// blur.
pub fn blit_mask(surf: &mut Surface, mask: &Mask, xform: &Transform, paint: &Paint) {
    if mask.w == 0 || mask.h == 0 {
        return;
    }
    let clip = paint.clip.intersect(&Rect::surface(surf.w, surf.h));
    if clip.is_empty() {
        return;
    }

    if let Some((ox, oy)) = axis_aligned_offset(xform) {
        let rect = Rect::new(ox as f64, oy as f64, mask.w as f64, mask.h as f64);
        let (x0, y0, x1, y1) = rect.intersect(&clip).pixel_bounds(surf.w, surf.h);
        for py in y0..y1 {
            let src_row = (py as i64 - oy) as usize * mask.w;
            let dst_row = py * surf.w;
            for px in x0..x1 {
                let a = mask.data[src_row + (px as i64 - ox) as usize];
                if a > 0 {
                    surf.blend(dst_row + px, paint.color, a as f32 / 255.0, paint.blend);
                }
            }
        }
        return;
    }

    let Some(inv) = xform.inverse() else { return };
    let bounds = transformed_bounds(xform, mask.w as f64, mask.h as f64);
    let (x0, y0, x1, y1) = bounds.intersect(&clip).pixel_bounds(surf.w, surf.h);

    for py in y0..y1 {
        let dst_row = py * surf.w;
        for px in x0..x1 {
            let (u, v) = inv.apply(px as f64 + 0.5, py as f64 + 0.5);
            let a = if paint.linear_filter {
                sample_mask_linear(mask, u, v)
            } else {
                sample_mask_nearest(mask, u, v)
            };
            if a > 0.0 {
                surf.blend(dst_row + px, paint.color, a, paint.blend);
            }
        }
    }
}

/// Draw one surface onto another through `xform`, modulated by the paint
/// colour. This is how a Canvas is composited back onto the screen.
pub fn blit_surface(dst: &mut Surface, src: &Surface, xform: &Transform, paint: &Paint) {
    if src.w == 0 || src.h == 0 {
        return;
    }
    let clip = paint.clip.intersect(&Rect::surface(dst.w, dst.h));
    if clip.is_empty() {
        return;
    }
    let Some(inv) = xform.inverse() else { return };
    let bounds = transformed_bounds(xform, src.w as f64, src.h as f64);
    let (x0, y0, x1, y1) = bounds.intersect(&clip).pixel_bounds(dst.w, dst.h);

    for py in y0..y1 {
        let dst_row = py * dst.w;
        for px in x0..x1 {
            let (u, v) = inv.apply(px as f64 + 0.5, py as f64 + 0.5);
            // Reject outside the source rect rather than clamping, so a
            // rotated canvas has clean edges instead of smeared borders.
            if u < 0.0 || v < 0.0 || u >= src.w as f64 || v >= src.h as f64 {
                continue;
            }
            let (sa, sr, sg, sb) = if paint.linear_filter {
                src.sample_linear(u, v)
            } else {
                src.sample_nearest(u, v)
            };
            if sa <= 0.0 {
                continue;
            }
            let tint = [
                sr * paint.color[0],
                sg * paint.color[1],
                sb * paint.color[2],
                sa * paint.color[3],
            ];
            dst.blend(dst_row + px, tint, 1.0, paint.blend);
        }
    }
}

/// Recognise a pure integer translation, the case a direct copy is valid for.
fn axis_aligned_offset(t: &Transform) -> Option<(i64, i64)> {
    const EPS: f64 = 1e-6;
    let unit = (t.a - 1.0).abs() < EPS
        && (t.d - 1.0).abs() < EPS
        && t.b.abs() < EPS
        && t.c.abs() < EPS;
    if !unit {
        return None;
    }
    if (t.tx - t.tx.round()).abs() < EPS && (t.ty - t.ty.round()).abs() < EPS {
        Some((t.tx.round() as i64, t.ty.round() as i64))
    } else {
        None
    }
}

/// The device-space axis-aligned bounds of a `w × h` rect under `xform`.
fn transformed_bounds(xform: &Transform, w: f64, h: f64) -> Rect {
    let corners = [
        xform.apply(0.0, 0.0),
        xform.apply(w, 0.0),
        xform.apply(w, h),
        xform.apply(0.0, h),
    ];
    let mut r = Rect {
        x0: f64::INFINITY,
        y0: f64::INFINITY,
        x1: f64::NEG_INFINITY,
        y1: f64::NEG_INFINITY,
    };
    for (x, y) in corners {
        r.x0 = r.x0.min(x);
        r.x1 = r.x1.max(x);
        r.y0 = r.y0.min(y);
        r.y1 = r.y1.max(y);
    }
    r
}

#[inline]
fn sample_mask_nearest(mask: &Mask, x: f64, y: f64) -> f32 {
    let (xi, yi) = (x.floor() as i64, y.floor() as i64);
    if xi < 0 || yi < 0 || xi as usize >= mask.w || yi as usize >= mask.h {
        return 0.0;
    }
    mask.data[yi as usize * mask.w + xi as usize] as f32 / 255.0
}

#[inline]
fn sample_mask_linear(mask: &Mask, x: f64, y: f64) -> f32 {
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
    let p = |dx: f64, dy: f64| sample_mask_nearest(mask, x0 + dx + 0.5, y0 + dy + 0.5);
    let top = p(0.0, 0.0) + (p(1.0, 0.0) - p(0.0, 0.0)) * tx;
    let bot = p(0.0, 1.0) + (p(1.0, 1.0) - p(0.0, 1.0)) * tx;
    top + (bot - top) * ty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint(color: [f32; 4], w: usize, h: usize) -> Paint {
        Paint {
            color,
            blend: BlendMode::Alpha,
            clip: Rect::surface(w, h),
            antialias: true,
            linear_filter: false,
        }
    }

    fn alpha_at(s: &Surface, x: usize, y: usize) -> f32 {
        unpack(s.buf[y * s.w + x]).0
    }

    fn red_at(s: &Surface, x: usize, y: usize) -> f32 {
        unpack(s.buf[y * s.w + x]).1
    }

    #[test]
    fn pack_unpack_round_trip() {
        let p = pack(1.0, 0.5, 0.25, 0.0);
        let (a, r, g, b) = unpack(p);
        assert!((a - 1.0).abs() < 0.01);
        assert!((r - 0.5).abs() < 0.01);
        assert!((g - 0.25).abs() < 0.01);
        assert!(b < 0.01);
    }

    #[test]
    fn aligned_rect_fills_exactly_and_leaves_neighbours_alone() {
        let mut s = Surface::new(10, 10);
        let rect = vec![vec![(2.0, 2.0), (6.0, 2.0), (6.0, 6.0), (2.0, 6.0)]];
        fill_paths(&mut s, &rect, &paint([1.0, 0.0, 0.0, 1.0], 10, 10));

        assert!((alpha_at(&s, 3, 3) - 1.0).abs() < 0.01, "interior not opaque");
        assert!((red_at(&s, 3, 3) - 1.0).abs() < 0.01, "wrong colour");
        assert!(alpha_at(&s, 1, 3) < 0.01, "leaked left of the rect");
        assert!(alpha_at(&s, 6, 3) < 0.01, "leaked right of the rect");
        assert!(alpha_at(&s, 3, 6) < 0.01, "leaked below the rect");
    }

    #[test]
    fn half_covered_pixel_gets_partial_coverage() {
        let mut s = Surface::new(4, 4);
        // Covers exactly the left half of column 1.
        let rect = vec![vec![(1.0, 0.0), (1.5, 0.0), (1.5, 4.0), (1.0, 4.0)]];
        fill_paths(&mut s, &rect, &paint([1.0, 1.0, 1.0, 1.0], 4, 4));
        let a = alpha_at(&s, 1, 1);
        assert!((a - 0.5).abs() < 0.02, "expected ~0.5 coverage, got {a}");
    }

    #[test]
    fn rough_mode_snaps_coverage_to_hard_edges() {
        let mut s = Surface::new(4, 4);
        let mut p = paint([1.0, 1.0, 1.0, 1.0], 4, 4);
        p.antialias = false;
        let rect = vec![vec![(1.0, 0.0), (1.6, 0.0), (1.6, 4.0), (1.0, 4.0)]];
        fill_paths(&mut s, &rect, &p);
        let a = alpha_at(&s, 1, 1);
        assert!((a - 1.0).abs() < 0.01, "expected snap to full, got {a}");
    }

    #[test]
    fn scissor_clips_the_fill() {
        let mut s = Surface::new(10, 10);
        let mut p = paint([1.0, 1.0, 1.0, 1.0], 10, 10);
        p.clip = Rect::new(0.0, 0.0, 5.0, 10.0);
        let rect = vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]];
        fill_paths(&mut s, &rect, &p);
        assert!(alpha_at(&s, 2, 2) > 0.9, "inside the scissor should be drawn");
        assert!(alpha_at(&s, 7, 2) < 0.01, "outside the scissor must be clipped");
    }

    #[test]
    fn overlapping_same_wound_polygons_union_without_double_blending() {
        // This is the property stroke joins depend on: two overlapping quads
        // at 50% alpha must read as one 50% shape, not 75%.
        let mut s = Surface::new(8, 8);
        let quad = |x: f64| vec![(x, 1.0), (x + 4.0, 1.0), (x + 4.0, 6.0), (x, 6.0)];
        let paths = vec![quad(1.0), quad(3.0)];
        fill_paths(&mut s, &paths, &paint([1.0, 1.0, 1.0, 0.5], 8, 8));
        let a = alpha_at(&s, 4, 3); // inside both quads
        assert!((a - 0.5).abs() < 0.02, "overlap double-blended: {a}");
    }

    #[test]
    fn replace_blend_overwrites_destination_alpha() {
        let mut s = Surface::new(4, 4);
        s.buf.fill(pack(1.0, 1.0, 1.0, 1.0));
        let mut p = paint([0.0, 0.0, 0.0, 0.0], 4, 4);
        p.blend = BlendMode::Replace;
        let rect = vec![vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]];
        fill_paths(&mut s, &rect, &p);
        assert!(alpha_at(&s, 2, 2) < 0.01, "replace should have zeroed alpha");
    }

    #[test]
    fn add_blend_accumulates_toward_white() {
        let mut s = Surface::new(4, 4);
        let mut p = paint([0.5, 0.0, 0.0, 1.0], 4, 4);
        p.blend = BlendMode::Add;
        let rect = vec![vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]];
        fill_paths(&mut s, &rect, &p);
        fill_paths(&mut s, &rect, &p);
        assert!((red_at(&s, 2, 2) - 1.0).abs() < 0.02, "two 0.5 adds should saturate");
    }

    #[test]
    fn clear_respects_the_scissor() {
        let mut s = Surface::new(8, 8);
        s.clear([1.0, 1.0, 1.0, 1.0], Rect::new(0.0, 0.0, 4.0, 8.0));
        assert!(alpha_at(&s, 1, 1) > 0.9);
        assert!(alpha_at(&s, 6, 1) < 0.01);
    }

    #[test]
    fn mask_blit_at_integer_offset_copies_exactly() {
        let mut s = Surface::new(8, 8);
        let mask = Mask {
            data: vec![255; 4],
            w: 2,
            h: 2,
        };
        blit_mask(
            &mut s,
            &mask,
            &Transform::translation(3.0, 3.0),
            &paint([1.0, 1.0, 1.0, 1.0], 8, 8),
        );
        assert!((alpha_at(&s, 3, 3) - 1.0).abs() < 0.01);
        assert!((alpha_at(&s, 4, 4) - 1.0).abs() < 0.01);
        assert!(alpha_at(&s, 5, 5) < 0.01, "mask wrote outside its bounds");
    }

    #[test]
    fn mask_blit_is_clipped_by_the_scissor() {
        let mut s = Surface::new(8, 8);
        let mask = Mask {
            data: vec![255; 16],
            w: 4,
            h: 4,
        };
        let mut p = paint([1.0, 1.0, 1.0, 1.0], 8, 8);
        p.clip = Rect::new(0.0, 0.0, 3.0, 8.0);
        blit_mask(&mut s, &mask, &Transform::translation(1.0, 1.0), &p);
        assert!(alpha_at(&s, 2, 2) > 0.9);
        assert!(alpha_at(&s, 4, 2) < 0.01);
    }

    #[test]
    fn surface_blit_copies_pixels_and_skips_transparent_source() {
        let mut dst = Surface::new(8, 8);
        let mut src = Surface::new(2, 2);
        src.buf[0] = pack(1.0, 1.0, 0.0, 0.0); // opaque red
        // remaining source pixels stay fully transparent

        blit_surface(
            &mut dst,
            &src,
            &Transform::translation(2.0, 2.0),
            &paint([1.0, 1.0, 1.0, 1.0], 8, 8),
        );
        assert!((red_at(&dst, 2, 2) - 1.0).abs() < 0.02, "opaque pixel not copied");
        assert!(alpha_at(&dst, 3, 2) < 0.01, "transparent source pixel wrote through");
    }

    #[test]
    fn rect_intersection_can_go_empty() {
        let a = Rect::new(0.0, 0.0, 4.0, 4.0);
        let b = Rect::new(10.0, 10.0, 4.0, 4.0);
        assert!(a.intersect(&b).is_empty());
    }

    #[test]
    fn blend_mode_names_round_trip() {
        for name in ["alpha", "add", "subtract", "multiply", "screen", "replace"] {
            assert_eq!(BlendMode::parse(name).unwrap().name(), name);
        }
        assert!(BlendMode::parse("glow").is_err());
    }
}
