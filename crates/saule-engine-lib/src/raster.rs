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

    /// Alpha-composite an 8-bit source over pixel `idx` at coverage `cov8`.
    ///
    /// This is the integer twin of [`BlendMode::Alpha`]'s arm in
    /// [`Surface::blend`] — the same formula, but with no float conversions in
    /// the loop. Alpha is by far the most-used mode (every translucent fill,
    /// every glyph, every canvas composite), and the eight int↔float
    /// conversions the general path spends per pixel dominate it, so the modes
    /// part ways here and share only their arithmetic.
    ///
    /// Results can differ from the float path by one unit in the last place,
    /// which is below the precision the framebuffer can hold anyway.
    #[inline]
    fn blend_alpha8(&mut self, idx: usize, src: Src8, cov8: u32) {
        let sa = div255(src.a * cov8);
        if sa == 0 {
            return;
        }
        if sa == 255 {
            self.buf[idx] = src.packed();
            return;
        }
        self.buf[idx] = lerp_argb(src.opaque_packed(), self.buf[idx], sa);
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

/// Divide by 255 with round-to-nearest, without a division.
///
/// Exact for every product two 8-bit channels can make.
#[inline]
fn div255(x: u32) -> u32 {
    let x = x + 128;
    (x + (x >> 8)) >> 8
}

/// `src * a + dst * (255 - a)` across all four channels of two packed pixels.
///
/// Red and blue are held in one word and alpha and green in another, each
/// channel with an empty byte above it. A channel times an 8-bit weight tops
/// out at 65025, so it cannot carry into its neighbour — which means two
/// channels ride along with every multiply, and the rounding division by 255
/// is done on both at once.
///
/// `src`'s alpha byte is the source's *contribution*, so callers compositing a
/// colour at coverage pass 255 there and put the real weight in `a`.
#[inline]
fn lerp_argb(src: u32, dst: u32, a: u32) -> u32 {
    const LANES: u32 = 0x00FF_00FF;
    const HALF: u32 = 0x0080_0080;
    let inv = 255 - a;

    let rb = (src & LANES) * a + (dst & LANES) * inv + HALF;
    let rb = ((rb + ((rb >> 8) & LANES)) >> 8) & LANES;

    let ag = ((src >> 8) & LANES) * a + ((dst >> 8) & LANES) * inv + HALF;
    let ag = ((ag + ((ag >> 8) & LANES)) >> 8) & LANES;

    (ag << 8) | rb
}

/// A colour quantized to 8 bits per channel, hoisted out of a pixel loop.
///
/// Converting the paint colour once per draw call rather than once per pixel is
/// most of what makes [`Surface::blend_alpha8`] worth having.
#[derive(Clone, Copy)]
struct Src8 {
    a: u32,
    r: u32,
    g: u32,
    b: u32,
}

impl Src8 {
    #[inline]
    fn new(color: [f32; 4]) -> Self {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        Src8 {
            a: q(color[3]),
            r: q(color[0]),
            g: q(color[1]),
            b: q(color[2]),
        }
    }

    /// Split a stored pixel back into channels.
    #[inline]
    fn from_pixel(p: u32) -> Self {
        Src8 {
            a: (p >> 24) & 0xFF,
            r: (p >> 16) & 0xFF,
            g: (p >> 8) & 0xFF,
            b: p & 0xFF,
        }
    }

    /// Modulate by another colour — how a blit applies its paint tint.
    #[inline]
    fn modulate(self, t: Src8) -> Self {
        Src8 {
            a: div255(self.a * t.a),
            r: div255(self.r * t.r),
            g: div255(self.g * t.g),
            b: div255(self.b * t.b),
        }
    }

    #[inline]
    fn is_opaque_white(self) -> bool {
        self.a == 255 && self.r == 255 && self.g == 255 && self.b == 255
    }

    #[inline]
    fn packed(self) -> u32 {
        (self.a << 24) | (self.r << 16) | (self.g << 8) | self.b
    }

    /// The colour with a full alpha byte — what [`lerp_argb`] wants, since the
    /// weight travels separately and the alpha channel composites as if the
    /// source were solid there.
    #[inline]
    fn opaque_packed(self) -> u32 {
        self.packed() | 0xFF00_0000
    }
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

    /// The packed pixel a fully covered sample resolves to, when the paint is
    /// opaque and its blend mode doesn't read the destination.
    ///
    /// Those are the paints whose interior pixels are a straight overwrite, so
    /// a run of them can be filled in one go instead of blended one at a time.
    /// Returns `None` for anything that has to go through [`Surface::blend`].
    #[inline]
    fn opaque_pixel(&self) -> Option<u32> {
        if self.color[3] < 1.0 {
            return None;
        }
        match self.blend {
            BlendMode::Alpha | BlendMode::Replace => Some(pack(
                self.color[3],
                self.color[0],
                self.color[1],
                self.color[2],
            )),
            _ => None,
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

    if let Some(rect) = axis_aligned_rect(paths) {
        fill_axis_aligned_rect(surf, rect, paint, clip);
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
    edges.sort_by(|a, b| {
        a.y_top
            .partial_cmp(&b.y_top)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let width = px1 - px0;
    let mut coverage = vec![0.0f32; width];
    let mut crossings: Vec<(f64, i32)> = Vec::with_capacity(16);
    let mut active: Vec<usize> = Vec::with_capacity(16);
    // The column ranges the current row actually covers.
    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(16);
    let mut cursor = 0usize;

    let sub_weight = 1.0 / SUBSAMPLES as f32;
    let solid = paint.opaque_pixel();
    // Alpha is the only mode with an integer implementation; the rest keep the
    // general float path.
    let src8 = (paint.blend == BlendMode::Alpha).then(|| Src8::new(paint.color));
    if src8.is_some_and(|s| s.a == 0) {
        return; // fully transparent alpha paint writes nothing
    }

    for py in py0..py1 {
        // The column ranges this row actually covers. A hollow shape — a
        // border, a focus ring, an outlined card — covers only a sliver at each
        // edge while its *envelope* is the whole width, so these have to stay
        // separate rather than collapse into one range. Both the blend scan and
        // the clear then skip the empty middle, which is the difference between
        // a 1px outline costing a sliver and costing the whole rectangle it
        // encloses.
        spans.clear();
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
                    let covered = add_span(
                        &mut coverage,
                        px0,
                        px1,
                        span_start.max(clip.x0),
                        x.min(clip.x1),
                        sub_weight,
                    );

                    if let Some(range) = covered {
                        spans.push(range);
                    }
                }
            }
        }

        if spans.is_empty() {
            continue; // this row is entirely outside the shape
        }
        merge_spans(&mut spans);

        let row = py * surf.w;
        for &(lo, hi) in spans.iter() {
            match solid {
                // Opaque paint: the fully covered interior is a memory fill,
                // and only the feathered edge pixels need a real blend. On a
                // large shape that is the difference between one blend per
                // pixel and a handful per row.
                Some(px) => {
                    let mut i = lo;
                    while i < hi {
                        if paint.shape(coverage[i]) >= 1.0 {
                            let start = i;
                            while i < hi && paint.shape(coverage[i]) >= 1.0 {
                                i += 1;
                            }
                            surf.buf[row + px0 + start..row + px0 + i].fill(px);
                        } else {
                            let c = paint.shape(coverage[i]);
                            if c > 0.0 {
                                // Opaque `Replace` also lands here, and its
                                // edge pixels need the Replace formula, so this
                                // defers to the mode rather than assuming
                                // alpha.
                                match src8 {
                                    Some(s) => surf.blend_alpha8(row + px0 + i, s, cov8(c)),
                                    None => surf.blend(row + px0 + i, paint.color, c, paint.blend),
                                }
                            }
                            i += 1;
                        }
                    }
                }
                // Translucent paint — no run is a plain overwrite, so every
                // covered pixel is composited. Shadows and overlays live here.
                None => match src8 {
                    // A fully covered run all composites at the same weight, so
                    // hoisting it out leaves an inner loop with nothing in it
                    // but the blend itself.
                    Some(s) => {
                        let sp = s.opaque_packed();
                        let mut i = lo;
                        while i < hi {
                            let c = paint.shape(coverage[i]);
                            if c >= 1.0 {
                                let start = i;
                                while i < hi && paint.shape(coverage[i]) >= 1.0 {
                                    i += 1;
                                }
                                for px in &mut surf.buf[row + px0 + start..row + px0 + i] {
                                    *px = lerp_argb(sp, *px, s.a);
                                }
                            } else {
                                if c > 0.0 {
                                    surf.blend_alpha8(row + px0 + i, s, cov8(c));
                                }
                                i += 1;
                            }
                        }
                    }
                    None => {
                        for (i, &cov) in coverage[lo..hi].iter().enumerate() {
                            let c = paint.shape(cov);
                            if c > 0.0 {
                                surf.blend(row + px0 + lo + i, paint.color, c, paint.blend);
                            }
                        }
                    }
                },
            }

            // Hand the next row a zeroed buffer. Only what was just used can be
            // non-zero, so this costs the covered columns, not the bbox width.
            coverage[lo..hi].fill(0.0);
        }
    }
}

/// Composite a horizontal run of pixels that all share one coverage value.
///
/// `start` and `end` are absolute indices into the surface. Splitting this out
/// is what lets the rectangle path below write a whole row with one call.
#[inline]
fn blend_run(
    surf: &mut Surface,
    start: usize,
    end: usize,
    coverage: f32,
    paint: &Paint,
    solid: Option<u32>,
    src8: Option<Src8>,
) {
    if end <= start {
        return;
    }
    let c = paint.shape(coverage);
    if c <= 0.0 {
        return;
    }

    if c >= 1.0 {
        if let Some(px) = solid {
            surf.buf[start..end].fill(px);
            return;
        }
        if let Some(s) = src8 {
            let sp = s.opaque_packed();
            for px in &mut surf.buf[start..end] {
                *px = lerp_argb(sp, *px, s.a);
            }
            return;
        }
    }

    match src8 {
        Some(s) => {
            let q = cov8(c);
            for i in start..end {
                surf.blend_alpha8(i, s, q);
            }
        }
        None => {
            for i in start..end {
                surf.blend(i, paint.color, c, paint.blend);
            }
        }
    }
}

/// Recognise a lone axis-aligned rectangle among the paths to be filled.
///
/// Backgrounds, cards, dividers, selection bands, table cells — a UI is mostly
/// these, and they need none of the scanline machinery.
fn axis_aligned_rect(paths: &[Vec<Point>]) -> Option<Rect> {
    if paths.len() != 1 || paths[0].len() != 4 {
        return None;
    }
    let p = &paths[0];
    let (x0, y0) = p[0];
    let (x1, y1) = p[1];
    let (x2, y2) = p[2];
    let (x3, y3) = p[3];

    // Wound either way round, starting along either axis.
    let flat = (y0 == y1 && x1 == x2 && y2 == y3 && x3 == x0)
        || (x0 == x1 && y1 == y2 && x2 == x3 && y3 == y0);
    if !flat {
        return None;
    }

    let rect = Rect {
        x0: x0.min(x1).min(x2).min(x3),
        y0: y0.min(y1).min(y2).min(y3),
        x1: x0.max(x1).max(x2).max(x3),
        y1: y0.max(y1).max(y2).max(y3),
    };
    if !(rect.x0.is_finite() && rect.y0.is_finite() && rect.x1.is_finite() && rect.y1.is_finite()) {
        return None;
    }

    Some(rect)
}

/// Fill an axis-aligned rectangle directly.
///
/// Every row is three runs — a partial left pixel, a solid middle, a partial
/// right pixel — so there is nothing to sort, no active edge list, and no
/// per-pixel coverage buffer. The middle of an opaque rectangle becomes a
/// single memory fill per row.
///
/// Vertical coverage is counted in the same [`SUBSAMPLES`] steps the scanline
/// filler uses and horizontal overlap is exact in both, so this produces
/// byte-identical output — it is purely a shorter route to the same pixels.
fn fill_axis_aligned_rect(surf: &mut Surface, rect: Rect, paint: &Paint, clip: Rect) {
    let r = rect.intersect(&clip);
    if r.is_empty() {
        return;
    }
    let (px0, py0, px1, py1) = r.pixel_bounds(surf.w, surf.h);
    if px1 <= px0 || py1 <= py0 {
        return;
    }

    let solid = paint.opaque_pixel();
    let src8 = (paint.blend == BlendMode::Alpha).then(|| Src8::new(paint.color));
    if src8.is_some_and(|s| s.a == 0) {
        return;
    }

    // Horizontal layout of a row, shared by all of them.
    let first = px0;
    let last = px1 - 1;
    let left_cov = ((first + 1) as f64).min(r.x1) - (first as f64).max(r.x0);
    let right_cov = ((last + 1) as f64).min(r.x1) - (last as f64).max(r.x0);
    let mid_start = (first + 1).min(px1);
    let mid_end = last.max(mid_start);

    for py in py0..py1 {
        let mut inside = 0;
        for s in 0..SUBSAMPLES {
            let sy = py as f64 + (s as f64 + 0.5) / SUBSAMPLES as f64;
            if sy >= r.y0 && sy < r.y1 {
                inside += 1;
            }
        }
        if inside == 0 {
            continue;
        }
        let vcov = inside as f32 / SUBSAMPLES as f32;
        let row = py * surf.w;

        if first == last {
            // A rectangle less than two pixels wide is only its edge column.
            blend_run(
                surf,
                row + first,
                row + first + 1,
                vcov * (r.x1 - r.x0).min(1.0) as f32,
                paint,
                solid,
                src8,
            );
            continue;
        }

        blend_run(
            surf,
            row + first,
            row + first + 1,
            vcov * left_cov as f32,
            paint,
            solid,
            src8,
        );
        blend_run(
            surf,
            row + mid_start,
            row + mid_end,
            vcov,
            paint,
            solid,
            src8,
        );
        blend_run(
            surf,
            row + last,
            row + last + 1,
            vcov * right_cov as f32,
            paint,
            solid,
            src8,
        );
    }
}

/// Sort and coalesce a row's column ranges in place.
///
/// Every sub-scanline contributes its own ranges and they overlap heavily — the
/// four samples of a solid shape give four near-identical ones — so collapsing
/// them first keeps each covered column blended exactly once.
fn merge_spans(spans: &mut Vec<(usize, usize)>) {
    spans.sort_unstable();

    let mut write = 0;
    for read in 1..spans.len() {
        if spans[read].0 <= spans[write].1 {
            spans[write].1 = spans[write].1.max(spans[read].1);
        } else {
            write += 1;
            spans[write] = spans[read];
        }
    }
    spans.truncate(write + 1);
}

/// Quantize a coverage weight to the 0..=255 the integer blender takes.
#[inline]
fn cov8(c: f32) -> u32 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u32
}

/// Accumulate a horizontal span's exact per-pixel overlap into `coverage`,
/// returning the half-open range of `coverage` indices it touched.
///
/// Only the two end pixels can be partially covered; everything between them
/// takes the whole `weight`. Splitting the span that way keeps the interior —
/// which is nearly all of it on a wide shape — a flat add that vectorizes,
/// instead of a per-pixel clamp against the span bounds.
#[inline]
fn add_span(
    coverage: &mut [f32],
    px0: usize,
    px1: usize,
    x_start: f64,
    x_end: f64,
    weight: f32,
) -> Option<(usize, usize)> {
    let lo = x_start.max(px0 as f64);
    let hi = x_end.min(px1 as f64);
    if hi <= lo {
        return None;
    }
    let first = lo.floor() as usize;
    let last = (hi.ceil() as usize).min(px1);
    if last <= first {
        return None;
    }
    let touched = Some((first - px0, last - px0));
    if last - first == 1 {
        coverage[first - px0] += (hi - lo) as f32 * weight;
        return touched;
    }

    coverage[first - px0] += ((first + 1) as f64 - lo) as f32 * weight;

    let inner_end = (hi.floor() as usize).min(last);
    for c in &mut coverage[first + 1 - px0..inner_end - px0] {
        *c += weight;
    }

    if inner_end < last {
        coverage[inner_end - px0] += (hi - inner_end as f64) as f32 * weight;
    }

    touched
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

    // The mask's own bytes are already 0..=255 coverage, so on the alpha path
    // a glyph never touches floating point at all.
    let src8 = (paint.blend == BlendMode::Alpha).then(|| Src8::new(paint.color));

    if let Some((ox, oy)) = axis_aligned_offset(xform) {
        let rect = Rect::new(ox as f64, oy as f64, mask.w as f64, mask.h as f64);
        let (x0, y0, x1, y1) = rect.intersect(&clip).pixel_bounds(surf.w, surf.h);
        for py in y0..y1 {
            let src_row = (py as i64 - oy) as usize * mask.w;
            let dst_row = py * surf.w;
            for px in x0..x1 {
                let a = mask.data[src_row + (px as i64 - ox) as usize];
                if a > 0 {
                    match src8 {
                        Some(s) => surf.blend_alpha8(dst_row + px, s, a as u32),
                        None => {
                            surf.blend(dst_row + px, paint.color, a as f32 / 255.0, paint.blend)
                        }
                    }
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
    let whole = Rect::new(0.0, 0.0, src.w as f64, src.h as f64);
    blit_surface_sub(dst, src, whole, xform, paint);
}

/// Draw the `sub` region of `src` onto `dst` through `xform`, with `sub`'s
/// top-left mapping to the transform's origin.
///
/// This is the spritesheet path: `xform` positions and scales the destination
/// while `sub` picks the frame, so one image can hold a whole animation.
pub fn blit_surface_sub(
    dst: &mut Surface,
    src: &Surface,
    sub: Rect,
    xform: &Transform,
    paint: &Paint,
) {
    // Confine the region to the source: a frame rectangle that runs off the
    // edge of the sheet should draw the part that exists, not sample garbage.
    let sub = sub.intersect(&Rect::surface(src.w, src.h));
    let (sub_w, sub_h) = (sub.x1 - sub.x0, sub.y1 - sub.y0);
    if src.w == 0 || src.h == 0 || sub_w <= 0.0 || sub_h <= 0.0 {
        return;
    }
    let clip = paint.clip.intersect(&Rect::surface(dst.w, dst.h));
    if clip.is_empty() {
        return;
    }

    // 1:1 at an integer offset — an overlay layer composited back over the
    // screen, which is the case that actually costs a full frame's worth of
    // pixels. Nothing here needs sampling: source and destination pixels
    // correspond exactly, so the inverse transform, the bounds rejection and
    // the half-pixel inset all fall away, and a fully opaque row is a memcpy.
    if let Some((ox, oy)) =
        axis_aligned_offset(xform).filter(|_| is_integral(&sub) && paint.blend == BlendMode::Alpha)
    {
        let tint = Src8::new(paint.color);
        if tint.a == 0 {
            return;
        }
        let rect = Rect::new(ox as f64, oy as f64, sub_w, sub_h);
        let (x0, y0, x1, y1) = rect.intersect(&clip).pixel_bounds(dst.w, dst.h);
        let (sx0, sy0) = (sub.x0 as i64, sub.y0 as i64);
        // Untinted is the overwhelmingly common case — a layer composited
        // as-is — and it skips the channel-wise modulate entirely, so the
        // branch is hoisted out of the row rather than tested per pixel.
        let plain = tint.is_opaque_white();

        for py in y0..y1 {
            let src_base =
                (py as i64 - oy + sy0) as usize * src.w + (x0 as i64 - ox + sx0) as usize;
            let dst_base = py * dst.w + x0;

            if plain {
                for k in 0..x1 - x0 {
                    let s = src.buf[src_base + k];
                    let sa = s >> 24;
                    if sa == 0 {
                        continue;
                    }
                    dst.buf[dst_base + k] = if sa == 255 {
                        s
                    } else {
                        lerp_argb(s | 0xFF00_0000, dst.buf[dst_base + k], sa)
                    };
                }
            } else {
                for k in 0..x1 - x0 {
                    let s = src.buf[src_base + k];
                    if s >> 24 == 0 {
                        continue;
                    }
                    dst.blend_alpha8(dst_base + k, Src8::from_pixel(s).modulate(tint), 255);
                }
            }
        }
        return;
    }

    let Some(inv) = xform.inverse() else { return };
    let bounds = transformed_bounds(xform, sub_w, sub_h);
    let (x0, y0, x1, y1) = bounds.intersect(&clip).pixel_bounds(dst.w, dst.h);

    for py in y0..y1 {
        let dst_row = py * dst.w;
        for px in x0..x1 {
            let (u, v) = inv.apply(px as f64 + 0.5, py as f64 + 0.5);
            // Reject outside the source rect rather than clamping, so a
            // rotated canvas has clean edges instead of smeared borders.
            if u < 0.0 || v < 0.0 || u >= sub_w || v >= sub_h {
                continue;
            }
            // Keep bilinear taps half a pixel inside the region: on a
            // spritesheet, sampling the frame's edge would otherwise pull in
            // the neighbouring frame. At 1:1 the samples already land on pixel
            // centres, so this is a no-op for an ordinary canvas blit.
            let u = sub.x0 + clamp_inside(u, sub_w);
            let v = sub.y0 + clamp_inside(v, sub_h);
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

/// Whether a source region lands on whole pixels, so it can be indexed
/// directly instead of sampled.
fn is_integral(r: &Rect) -> bool {
    r.x0.fract() == 0.0 && r.y0.fract() == 0.0 && r.x1.fract() == 0.0 && r.y1.fract() == 0.0
}

/// Pin a source coordinate to the half-pixel-inset interior of a `0..extent`
/// span, so a bilinear tap can't reach past the region being sampled.
fn clamp_inside(value: f64, extent: f64) -> f64 {
    if extent <= 1.0 {
        return extent / 2.0;
    }
    value.clamp(0.5, extent - 0.5)
}

/// Recognise a pure integer translation, the case a direct copy is valid for.
fn axis_aligned_offset(t: &Transform) -> Option<(i64, i64)> {
    const EPS: f64 = 1e-6;
    let unit =
        (t.a - 1.0).abs() < EPS && (t.d - 1.0).abs() < EPS && t.b.abs() < EPS && t.c.abs() < EPS;
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

    /// A one-pixel-tall strip of four distinct opaque colours — a spritesheet
    /// with four 1x1 frames.
    fn strip() -> Surface {
        Surface {
            buf: vec![0xFFFF_0000, 0xFF00_FF00, 0xFF00_00FF, 0xFFFF_FFFF],
            w: 4,
            h: 1,
        }
    }

    #[test]
    fn a_frame_blit_picks_exactly_its_cell() {
        for (frame, expected) in [
            (0.0, 0xFFFF_0000u32),
            (1.0, 0xFF00_FF00),
            (2.0, 0xFF00_00FF),
            (3.0, 0xFFFF_FFFF),
        ] {
            let mut dst = Surface::new(1, 1);
            blit_surface_sub(
                &mut dst,
                &strip(),
                Rect::new(frame, 0.0, 1.0, 1.0),
                &Transform::IDENTITY,
                &paint([1.0, 1.0, 1.0, 1.0], 1, 1),
            );
            assert_eq!(dst.buf[0], expected, "frame {frame}");
        }
    }

    /// With bilinear filtering on, a magnified frame must not pull colour out of
    /// the neighbouring cell.
    #[test]
    fn a_magnified_frame_does_not_bleed_into_its_neighbour() {
        let mut dst = Surface::new(8, 8);
        let mut p = paint([1.0, 1.0, 1.0, 1.0], 8, 8);
        p.linear_filter = true;

        blit_surface_sub(
            &mut dst,
            &strip(),
            Rect::new(1.0, 0.0, 1.0, 1.0),
            &Transform::scaling(8.0, 8.0),
            &p,
        );

        // Every touched pixel is the green frame, with no red or blue mixed in.
        for (i, px) in dst.buf.iter().enumerate() {
            assert_eq!(*px, 0xFF00_FF00, "pixel {i} bled");
        }
    }

    #[test]
    fn a_frame_running_past_the_sheet_is_clipped_to_it() {
        let mut dst = Surface::new(4, 1);
        blit_surface_sub(
            &mut dst,
            &strip(),
            Rect::new(3.0, 0.0, 4.0, 1.0),
            &Transform::IDENTITY,
            &paint([1.0, 1.0, 1.0, 1.0], 4, 1),
        );

        // Only the one real column exists, so only one pixel is written.
        assert_eq!(dst.buf[0], 0xFFFF_FFFF);
        assert_eq!(dst.buf[1], 0);
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

        assert!(
            (alpha_at(&s, 3, 3) - 1.0).abs() < 0.01,
            "interior not opaque"
        );
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
        assert!(
            alpha_at(&s, 2, 2) > 0.9,
            "inside the scissor should be drawn"
        );
        assert!(
            alpha_at(&s, 7, 2) < 0.01,
            "outside the scissor must be clipped"
        );
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
        assert!(
            alpha_at(&s, 2, 2) < 0.01,
            "replace should have zeroed alpha"
        );
    }

    #[test]
    fn add_blend_accumulates_toward_white() {
        let mut s = Surface::new(4, 4);
        let mut p = paint([0.5, 0.0, 0.0, 1.0], 4, 4);
        p.blend = BlendMode::Add;
        let rect = vec![vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]];
        fill_paths(&mut s, &rect, &p);
        fill_paths(&mut s, &rect, &p);
        assert!(
            (red_at(&s, 2, 2) - 1.0).abs() < 0.02,
            "two 0.5 adds should saturate"
        );
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
        assert!(
            (red_at(&dst, 2, 2) - 1.0).abs() < 0.02,
            "opaque pixel not copied"
        );
        assert!(
            alpha_at(&dst, 3, 2) < 0.01,
            "transparent source pixel wrote through"
        );
    }

    #[test]
    fn rect_intersection_can_go_empty() {
        let a = Rect::new(0.0, 0.0, 4.0, 4.0);
        let b = Rect::new(10.0, 10.0, 4.0, 4.0);
        assert!(a.intersect(&b).is_empty());
    }

    /// Largest per-channel difference between two surfaces.
    fn max_channel_delta(a: &Surface, b: &Surface) -> (u32, usize) {
        let mut worst = (0, 0);
        for i in 0..a.buf.len() {
            for shift in [24, 16, 8, 0] {
                let l = (a.buf[i] >> shift) & 0xFF;
                let r = (b.buf[i] >> shift) & 0xFF;
                if l.abs_diff(r) > worst.0 {
                    worst = (l.abs_diff(r), i);
                }
            }
        }
        worst
    }

    /// A shape rotated 45° so every row has feathered edges — the fast paths
    /// have to hand those partial pixels back to the blender rather than
    /// snapping them solid.
    fn diamond() -> Vec<Vec<Point>> {
        vec![vec![(9.0, 2.0), (17.0, 10.0), (9.0, 18.0), (1.0, 10.0)]]
    }

    /// The opaque run-fill writes the packed colour straight in, so it has to
    /// agree with the general blender exactly — no rounding slack at all.
    #[test]
    fn the_opaque_fast_path_matches_a_blended_fill_exactly() {
        let mut fast = Surface::new(20, 20);
        let p = paint([0.2, 0.6, 0.9, 1.0], 20, 20);
        fill_paths(&mut fast, &diamond(), &p);

        // Same coverage, but forced down the per-pixel float branch by a mode
        // the fast path always declines. Over transparent black, Screen and
        // Alpha reduce to the same thing.
        let mut reference = Surface::new(20, 20);
        let mut slow = p;
        slow.blend = BlendMode::Screen;
        assert!(slow.opaque_pixel().is_none(), "control must stay generic");
        fill_paths(&mut reference, &diamond(), &slow);

        let (delta, at) = max_channel_delta(&fast, &reference);
        assert_eq!(delta, 0, "opaque fill differs at pixel {at}");
    }

    /// A translucent fill — a shadow or a scrim — composites in integer
    /// arithmetic, which is allowed to round differently from the float path
    /// but never by more than one unit in the last place.
    #[test]
    fn the_integer_alpha_blend_tracks_the_float_blend_within_one_lsb() {
        for alpha in [0.25, 0.5, 0.75] {
            let mut fast = Surface::opaque(20, 20);
            let p = paint([0.2, 0.6, 0.9, alpha], 20, 20);
            fill_paths(&mut fast, &diamond(), &p);

            let mut reference = Surface::opaque(20, 20);
            let mut slow = p;
            slow.blend = BlendMode::Screen;
            fill_paths(&mut reference, &diamond(), &slow);

            // Screen over an opaque *black* destination still reduces to alpha
            // compositing, so the two remain comparable.
            let (delta, at) = max_channel_delta(&fast, &reference);
            assert!(delta <= 1, "alpha {alpha}: off by {delta} at pixel {at}");
        }
    }

    /// The 1:1 canvas composite — an overlay layer drawn back over the screen —
    /// must land on the same pixels as the sampling path it short-circuits.
    #[test]
    fn the_direct_canvas_blit_matches_the_sampled_one() {
        // A layer with an opaque region, a translucent region, and holes.
        let mut layer = Surface::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                layer.buf[y * 16 + x] = match (x / 4 + y / 4) % 3 {
                    0 => 0,
                    1 => pack(1.0, 0.9, 0.3, 0.1),
                    _ => pack(0.5, 0.1, 0.4, 0.8),
                };
            }
        }

        for tint in [[1.0, 1.0, 1.0, 1.0], [1.0, 0.5, 0.5, 0.8]] {
            let mut direct = Surface::opaque(24, 24);
            let p = paint(tint, 24, 24);
            blit_surface(&mut direct, &layer, &Transform::translation(3.0, 5.0), &p);

            // Far enough off an integer offset to decline the direct path —
            // `axis_aligned_offset` tolerates 1e-6 — but far too small to move
            // which source pixel any destination pixel samples. So this goes
            // through the inverse transform and the sampler on the same pixels.
            let nudged = Transform::translation(3.0 + 1e-5, 5.0 + 1e-5);
            assert!(
                axis_aligned_offset(&nudged).is_none(),
                "the control has to decline the direct path, or this proves nothing"
            );
            let mut sampled = Surface::opaque(24, 24);
            blit_surface(&mut sampled, &layer, &nudged, &p);

            let (delta, at) = max_channel_delta(&direct, &sampled);
            assert!(delta <= 1, "tint {tint:?}: off by {delta} at pixel {at}");
        }
    }

    #[test]
    fn a_full_row_span_accumulates_exactly_one_unit_of_coverage() {
        // The split into leading partial / interior / trailing partial must not
        // drop or double-count a pixel at either boundary.
        let mut cov = vec![0.0f32; 6];
        add_span(&mut cov, 0, 6, 1.25, 4.5, 1.0);
        let expected = [0.0, 0.75, 1.0, 1.0, 0.5, 0.0];
        for (i, (got, want)) in cov.iter().zip(expected).enumerate() {
            assert!((got - want).abs() < 1e-6, "pixel {i}: {got} != {want}");
        }
    }

    /// The coverage buffer is reused across rows and only the part a row
    /// touched is cleared. A shape that narrows as it descends would expose a
    /// wrong clear immediately: the wide rows above would leave coverage behind
    /// in columns the narrow rows below never reach.
    #[test]
    fn a_narrowing_shape_leaves_no_stale_coverage_below() {
        let mut s = Surface::new(24, 24);
        // Right triangle: row 0 spans the full width, the last row barely one
        // pixel.
        let tri = vec![vec![(0.0, 0.0), (20.0, 0.0), (0.0, 20.0)]];
        fill_paths(&mut s, &tri, &paint([1.0, 1.0, 1.0, 1.0], 24, 24));

        for y in 0..20 {
            // Everything past the hypotenuse (x + y = 20) must be untouched.
            for x in (20 - y + 1)..24 {
                assert!(
                    alpha_at(&s, x, y) < 0.01,
                    "stale coverage at ({x},{y}) below a wider row"
                );
            }
        }
        assert!(
            alpha_at(&s, 1, 1) > 0.9,
            "the triangle itself should be drawn"
        );
    }

    /// A hollow shape is the case the row-extent tracking exists for. Its hole
    /// must stay untouched, and the band around it must still be solid.
    #[test]
    fn a_ring_fills_its_band_and_spares_its_hole() {
        let mut s = Surface::opaque(40, 40);
        // Outer ring wound one way, inner wound the other, so the nonzero rule
        // punches the hole out.
        let outer = vec![(4.0, 4.0), (36.0, 4.0), (36.0, 36.0), (4.0, 36.0)];
        let inner = vec![(10.0, 10.0), (10.0, 30.0), (30.0, 30.0), (30.0, 10.0)];
        let mut p = paint([1.0, 0.0, 0.0, 1.0], 40, 40);
        p.blend = BlendMode::Alpha;
        fill_paths(&mut s, &[outer, inner], &p);

        assert!(
            (red_at(&s, 6, 20) - 1.0).abs() < 0.01,
            "left band not filled"
        );
        assert!(
            (red_at(&s, 33, 20) - 1.0).abs() < 0.01,
            "right band not filled"
        );
        assert!(red_at(&s, 20, 20) < 0.01, "the hole was painted over");
        assert!(red_at(&s, 2, 20) < 0.01, "paint leaked outside the ring");
    }

    /// The rectangle path is a shortcut, not a different renderer: it has to
    /// land on exactly the pixels the scanline filler would have written.
    ///
    /// The control is the same rectangle with an extra collinear vertex — five
    /// points, so `axis_aligned_rect` declines it — which is geometrically
    /// identical but goes the long way round.
    #[test]
    fn the_rectangle_shortcut_is_byte_identical_to_the_scanline_filler() {
        let cases = [
            (2.0, 3.0, 12.0, 9.0),   // whole pixels
            (2.25, 3.5, 12.75, 9.5), // fractional on every edge
            (4.6, 4.4, 5.2, 11.9),   // narrower than two pixels
            (-3.0, -2.5, 8.0, 6.25), // starting off the surface
        ];

        for (x0, y0, x1, y1) in cases {
            for alpha in [1.0, 0.45] {
                let p = paint([0.3, 0.7, 0.2, alpha], 20, 20);

                let mut fast = Surface::opaque(20, 20);
                let quad = vec![vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]];
                assert!(
                    axis_aligned_rect(&quad).is_some(),
                    "should take the shortcut"
                );
                fill_paths(&mut fast, &quad, &p);

                let mut slow = Surface::opaque(20, 20);
                let midpoint = (x0 + x1) * 0.5;
                let split = vec![vec![(x0, y0), (midpoint, y0), (x1, y0), (x1, y1), (x0, y1)]];
                assert!(
                    axis_aligned_rect(&split).is_none(),
                    "the control must decline the shortcut"
                );
                fill_paths(&mut slow, &split, &p);

                let (delta, at) = max_channel_delta(&fast, &slow);
                assert_eq!(
                    delta, 0,
                    "rect ({x0},{y0})-({x1},{y1}) at alpha {alpha} differs at pixel {at}"
                );
            }
        }
    }

    #[test]
    fn the_rectangle_shortcut_declines_anything_that_is_not_one() {
        // A diamond: four points, none of the edges axis-aligned.
        let diamond = vec![vec![(5.0, 0.0), (10.0, 5.0), (5.0, 10.0), (0.0, 5.0)]];
        assert!(axis_aligned_rect(&diamond).is_none());

        // Two rectangles at once — the nonzero rule may punch a hole.
        let pair = vec![
            vec![(0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0)],
            vec![(2.0, 2.0), (2.0, 6.0), (6.0, 6.0), (6.0, 2.0)],
        ];
        assert!(axis_aligned_rect(&pair).is_none());
    }

    #[test]
    fn blend_mode_names_round_trip() {
        for name in ["alpha", "add", "subtract", "multiply", "screen", "replace"] {
            assert_eq!(BlendMode::parse(name).unwrap().name(), name);
        }
        assert!(BlendMode::parse("glow").is_err());
    }
}
