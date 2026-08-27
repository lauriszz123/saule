//! Pixel-level primitives: the ARGB surface, packing and unpacking,
//! colour interpolation, blend modes, rectangles and paints.

/// Sub-scanlines sampled per pixel row. Four is the usual quality/speed
/// sweet spot: vertical edges are exact either way, and near-horizontal ones
/// resolve to 5 distinct coverage levels.
pub(crate) const SUBSAMPLES: usize = 4;

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
    pub(crate) fn blend(&mut self, idx: usize, color: [f32; 4], coverage: f32, mode: BlendMode) {
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
    pub(crate) fn blend_alpha8(&mut self, idx: usize, src: Src8, cov8: u32) {
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
    pub(crate) fn sample_nearest(&self, x: f64, y: f64) -> (f32, f32, f32, f32) {
        let (xi, yi) = (x.floor() as i64, y.floor() as i64);
        if xi < 0 || yi < 0 || xi as usize >= self.w || yi as usize >= self.h {
            return (0.0, 0.0, 0.0, 0.0);
        }
        unpack(self.buf[yi as usize * self.w + xi as usize])
    }

    /// Bilinear sample at pixel-centre convention.
    #[inline]
    pub(crate) fn sample_linear(&self, x: f64, y: f64) -> (f32, f32, f32, f32) {
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
pub(crate) fn unpack(p: u32) -> (f32, f32, f32, f32) {
    const INV: f32 = 1.0 / 255.0;
    (
        ((p >> 24) & 0xFF) as f32 * INV,
        ((p >> 16) & 0xFF) as f32 * INV,
        ((p >> 8) & 0xFF) as f32 * INV,
        (p & 0xFF) as f32 * INV,
    )
}

#[inline]
pub(crate) fn pack(a: f32, r: f32, g: f32, b: f32) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (q(a) << 24) | (q(r) << 16) | (q(g) << 8) | q(b)
}

/// Divide by 255 with round-to-nearest, without a division.
///
/// Exact for every product two 8-bit channels can make.
#[inline]
pub(crate) fn div255(x: u32) -> u32 {
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
pub(crate) fn lerp_argb(src: u32, dst: u32, a: u32) -> u32 {
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
pub(crate) struct Src8 {
    pub(crate) a: u32,
    pub(crate) r: u32,
    pub(crate) g: u32,
    pub(crate) b: u32,
}

impl Src8 {
    #[inline]
    pub(crate) fn new(color: [f32; 4]) -> Self {
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
    pub(crate) fn from_pixel(p: u32) -> Self {
        Src8 {
            a: (p >> 24) & 0xFF,
            r: (p >> 16) & 0xFF,
            g: (p >> 8) & 0xFF,
            b: p & 0xFF,
        }
    }

    /// Modulate by another colour — how a blit applies its paint tint.
    #[inline]
    pub(crate) fn modulate(self, t: Src8) -> Self {
        Src8 {
            a: div255(self.a * t.a),
            r: div255(self.r * t.r),
            g: div255(self.g * t.g),
            b: div255(self.b * t.b),
        }
    }

    #[inline]
    pub(crate) fn is_opaque_white(self) -> bool {
        self.a == 255 && self.r == 255 && self.g == 255 && self.b == 255
    }

    #[inline]
    pub(crate) fn packed(self) -> u32 {
        (self.a << 24) | (self.r << 16) | (self.g << 8) | self.b
    }

    /// The colour with a full alpha byte — what [`lerp_argb`] wants, since the
    /// weight travels separately and the alpha channel composites as if the
    /// source were solid there.
    #[inline]
    pub(crate) fn opaque_packed(self) -> u32 {
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
    pub(crate) fn pixel_bounds(&self, w: usize, h: usize) -> (usize, usize, usize, usize) {
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
    /// Colour source for fills. `None` is the flat [`Paint::color`]; a
    /// gradient replaces it per pixel and gives up the solid-run fast paths.
    pub gradient: Option<Gradient>,
}

impl Paint {
    #[inline]
    pub(crate) fn shape(&self, coverage: f32) -> f32 {
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
    pub(crate) fn opaque_pixel(&self) -> Option<u32> {
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

// ---------------------------------------------------------------------------
// Gradients
// ---------------------------------------------------------------------------

/// The most colour stops a gradient can carry.
///
/// Fixed rather than growable so [`Paint`] stays `Copy` and a draw call still
/// costs one memcpy instead of a heap allocation. Eight covers every gradient
/// a UI actually draws; more than that is a texture.
pub const MAX_STOPS: usize = 8;

/// One colour stop: a position in `0.0..=1.0` and the colour there.
#[derive(Clone, Copy, Debug)]
pub struct Stop {
    pub at: f32,
    pub color: [f32; 4],
}

impl Default for Stop {
    fn default() -> Self {
        Stop {
            at: 0.0,
            color: [0.0, 0.0, 0.0, 0.0],
        }
    }
}

/// Where a gradient's parameter runs from and to, in device coordinates.
#[derive(Clone, Copy, Debug)]
pub enum GradientKind {
    /// `t` is the projection onto the axis from `(x0, y0)` to `(x1, y1)`.
    Linear { x0: f64, y0: f64, x1: f64, y1: f64 },
    /// `t` is the distance from `(cx, cy)` over `radius`.
    Radial { cx: f64, cy: f64, radius: f64 },
}

/// A colour ramp used as a fill source in place of a flat colour.
///
/// Geometry is device-space: [`crate::render::Renderer::set_gradient`] bakes
/// the current transform in when the gradient is set, so a gradient stays put
/// under a later `translate` exactly the way a scissor does.
#[derive(Clone, Copy, Debug)]
pub struct Gradient {
    pub kind: GradientKind,
    stops: [Stop; MAX_STOPS],
    count: u8,
}

impl Gradient {
    /// Build a gradient from its stops, which are sorted and clamped here so
    /// the sampler can assume they are ordered.
    pub fn new(kind: GradientKind, stops: &[Stop]) -> Result<Self, String> {
        if stops.len() < 2 {
            return Err(format!(
                "a gradient needs at least 2 colour stops, got {}",
                stops.len()
            ));
        }
        if stops.len() > MAX_STOPS {
            return Err(format!(
                "a gradient may have at most {MAX_STOPS} colour stops, got {}",
                stops.len()
            ));
        }

        let mut buf = [Stop::default(); MAX_STOPS];
        for (slot, stop) in buf.iter_mut().zip(stops) {
            *slot = Stop {
                at: stop.at.clamp(0.0, 1.0),
                color: stop.color,
            };
        }
        let count = stops.len();
        buf[..count].sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));

        Ok(Gradient {
            kind,
            stops: buf,
            count: count as u8,
        })
    }

    /// The same gradient with `t` mapped through a transform.
    pub fn transformed(&self, t: &crate::geom::Transform) -> Gradient {
        let kind = match self.kind {
            GradientKind::Linear { x0, y0, x1, y1 } => {
                let (x0, y0) = t.apply(x0, y0);
                let (x1, y1) = t.apply(x1, y1);
                GradientKind::Linear { x0, y0, x1, y1 }
            }
            GradientKind::Radial { cx, cy, radius } => {
                let (cx, cy) = t.apply(cx, cy);
                GradientKind::Radial {
                    cx,
                    cy,
                    // A non-uniform scale has no single radius; the isotropic
                    // estimate is the same one curve tessellation uses.
                    radius: radius * t.mean_scale(),
                }
            }
        };
        Gradient { kind, ..*self }
    }

    /// The gradient's parameter at a device point, clamped to `0.0..=1.0`.
    #[inline]
    fn parameter(&self, x: f64, y: f64) -> f32 {
        let t = match self.kind {
            GradientKind::Linear { x0, y0, x1, y1 } => {
                let (dx, dy) = (x1 - x0, y1 - y0);
                let len2 = dx * dx + dy * dy;
                if len2 <= 1e-12 {
                    0.0
                } else {
                    ((x - x0) * dx + (y - y0) * dy) / len2
                }
            }
            GradientKind::Radial { cx, cy, radius } => {
                if radius.abs() <= 1e-12 {
                    1.0
                } else {
                    ((x - cx).hypot(y - cy)) / radius
                }
            }
        };
        (t as f32).clamp(0.0, 1.0)
    }

    /// The colour at the centre of device pixel `(px, py)`.
    #[inline]
    pub fn sample(&self, px: usize, py: usize) -> [f32; 4] {
        let t = self.parameter(px as f64 + 0.5, py as f64 + 0.5);
        let stops = &self.stops[..self.count as usize];

        // Before the first stop and after the last, the ramp is flat.
        if t <= stops[0].at {
            return stops[0].color;
        }
        let last = stops[stops.len() - 1];
        if t >= last.at {
            return last.color;
        }

        for pair in stops.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t <= b.at {
                let span = b.at - a.at;
                let local = if span <= f32::EPSILON {
                    0.0
                } else {
                    (t - a.at) / span
                };
                let mut out = [0.0f32; 4];
                for (channel, (&from, &to)) in
                    out.iter_mut().zip(a.color.iter().zip(b.color.iter()))
                {
                    *channel = from + (to - from) * local;
                }
                return out;
            }
        }
        last.color
    }
}
