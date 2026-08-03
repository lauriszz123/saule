//! The drawing state machine: current colour and transform, the
//! save/restore stack, and the clear/draw entry points.

use crate::font::FontRes;
use crate::geom::{self, ArcType, LineJoin, Point, Transform};
use crate::raster::{self, BlendMode, Paint, Rect, Surface};

use super::*;

/// The retained graphics state — everything `Graphics.push("all")` saves and
/// `Graphics.reset()` restores.
#[derive(Clone)]
pub(crate) struct GState {
    /// Kept at `f64` so `getColor` returns exactly what `setColor` was given;
    /// it narrows to `f32` only when a [`Paint`] is built.
    pub(crate) color: [f64; 4],
    pub(crate) line_width: f64,
    pub(crate) line_join: LineJoin,
    /// `false` is `setLineStyle("rough")`: hard, aliased edges.
    pub(crate) smooth: bool,
    pub(crate) blend: BlendMode,
    /// Device-space clip. `None` means "the whole render target".
    pub(crate) scissor: Option<Rect>,
    pub(crate) transform: Transform,
    /// Index into [`Engine::fonts`]; `0` is the lazily-loaded system default.
    pub(crate) font: usize,
}

impl Default for GState {
    fn default() -> Self {
        GState {
            color: [1.0, 1.0, 1.0, 1.0],
            line_width: 1.0,
            line_join: LineJoin::Miter,
            smooth: true,
            blend: BlendMode::Alpha,
            scissor: None,
            transform: Transform::IDENTITY,
            font: 0,
        }
    }
}

/// One `Graphics.push` frame. A bare `push()` is transform-only (matching
/// Love2D); `push("all")` snapshots the whole state.
pub(crate) enum Saved {
    TransformOnly(Transform),
    All(Box<GState>),
}

impl Engine {
    pub fn set_color(&mut self, r: f64, g: f64, b: f64, a: f64) {
        self.st.color = [r, g, b, a];
    }

    pub fn color(&self) -> (f64, f64, f64, f64) {
        let c = self.st.color;
        (c[0], c[1], c[2], c[3])
    }

    pub fn set_background_color(&mut self, r: f64, g: f64, b: f64, a: f64) {
        self.background = [r, g, b, a];
    }

    pub fn background_color(&self) -> (f64, f64, f64, f64) {
        let c = self.background;
        (c[0], c[1], c[2], c[3])
    }

    pub fn set_line_width(&mut self, w: f64) {
        self.st.line_width = w.max(0.0);
    }

    pub fn line_width(&self) -> f64 {
        self.st.line_width
    }

    pub fn set_line_style(&mut self, style: &str) -> Result<(), String> {
        self.st.smooth = match style {
            "smooth" => true,
            "rough" => false,
            other => {
                return Err(format!(
                    "Graphics.setLineStyle: unknown style `{other}` \
                     (expected \"smooth\" or \"rough\")"
                ));
            }
        };
        Ok(())
    }

    pub fn line_style(&self) -> &'static str {
        if self.st.smooth { "smooth" } else { "rough" }
    }

    pub fn set_line_join(&mut self, join: &str) -> Result<(), String> {
        self.st.line_join =
            LineJoin::parse(join).map_err(|e| format!("Graphics.setLineJoin: {e}"))?;
        Ok(())
    }

    pub fn line_join(&self) -> &'static str {
        self.st.line_join.name()
    }

    pub fn set_blend_mode(&mut self, mode: &str) -> Result<(), String> {
        self.st.blend =
            BlendMode::parse(mode).map_err(|e| format!("Graphics.setBlendMode: {e}"))?;
        Ok(())
    }

    pub fn blend_mode(&self) -> &'static str {
        self.st.blend.name()
    }

    pub fn set_default_filter(&mut self, mode: &str) -> Result<(), String> {
        self.linear_filter = match mode {
            "linear" => true,
            "nearest" => false,
            other => {
                return Err(format!(
                    "Graphics.setDefaultFilter: unknown filter `{other}` \
                     (expected \"linear\" or \"nearest\")"
                ));
            }
        };
        Ok(())
    }

    pub fn default_filter(&self) -> &'static str {
        if self.linear_filter {
            "linear"
        } else {
            "nearest"
        }
    }

    /// The device-space bounding box of a local rectangle under the current
    /// transform. Scissors follow the transform, so a clipped scroll view keeps
    /// working when its parent is translated.
    pub(crate) fn device_rect(&self, x: f64, y: f64, w: f64, h: f64) -> Rect {
        let t = &self.st.transform;
        let corners = [
            t.apply(x, y),
            t.apply(x + w, y),
            t.apply(x + w, y + h),
            t.apply(x, y + h),
        ];
        let mut r = Rect {
            x0: f64::INFINITY,
            y0: f64::INFINITY,
            x1: f64::NEG_INFINITY,
            y1: f64::NEG_INFINITY,
        };
        for (px, py) in corners {
            r.x0 = r.x0.min(px);
            r.x1 = r.x1.max(px);
            r.y0 = r.y0.min(py);
            r.y1 = r.y1.max(py);
        }
        r
    }

    /// `None` disables clipping entirely.
    pub fn set_scissor(&mut self, rect: Option<(f64, f64, f64, f64)>) {
        let device = rect.map(|(x, y, w, h)| self.device_rect(x, y, w, h));
        self.st.scissor = device;
    }

    /// Narrow the clip to the intersection with an existing one. This is what
    /// makes nested clipping — a scroll view inside a scroll view — compose.
    pub fn intersect_scissor(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let device = self.device_rect(x, y, w, h);
        self.st.scissor = Some(match self.st.scissor {
            Some(existing) => existing.intersect(&device),
            None => device,
        });
    }

    /// The active clip in device coordinates, defaulting to the whole target.
    pub fn scissor(&self) -> (f64, f64, f64, f64) {
        let (w, h) = self.target_size();
        let r = self.st.scissor.unwrap_or(Rect::surface(w, h));
        (r.x0, r.y0, r.x1 - r.x0, r.y1 - r.y0)
    }

    /// Restore every default: colour, line settings, blend mode, filter,
    /// scissor, transform, and font selection. Canvases and loaded fonts
    /// survive, since they are resources rather than state.
    pub fn reset(&mut self) {
        self.st = GState::default();
        self.stack.clear();
        self.background = [0.0, 0.0, 0.0, 1.0];
        self.linear_filter = true;
        self.target = None;
    }
}

// ---------------------------------------------------------------------------
// Coordinate system
// ---------------------------------------------------------------------------

impl Engine {
    pub fn push(&mut self, all: bool) {
        self.stack.push(if all {
            Saved::All(Box::new(self.st.clone()))
        } else {
            Saved::TransformOnly(self.st.transform)
        });
    }

    pub fn pop(&mut self) -> Result<(), String> {
        match self.stack.pop() {
            Some(Saved::TransformOnly(t)) => self.st.transform = t,
            Some(Saved::All(s)) => self.st = *s,
            None => return Err("Graphics.pop: the transform stack is empty".into()),
        }
        Ok(())
    }

    pub fn stack_depth(&self) -> i64 {
        self.stack.len() as i64
    }

    pub fn origin(&mut self) {
        self.st.transform = Transform::IDENTITY;
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        self.st.transform = self.st.transform.then(&Transform::translation(dx, dy));
    }

    pub fn scale(&mut self, sx: f64, sy: f64) {
        self.st.transform = self.st.transform.then(&Transform::scaling(sx, sy));
    }

    pub fn rotate(&mut self, angle: f64) {
        self.st.transform = self.st.transform.then(&Transform::rotation(angle));
    }

    pub fn shear(&mut self, kx: f64, ky: f64) {
        self.st.transform = self.st.transform.then(&Transform::shearing(kx, ky));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_transform(&mut self, a: f64, b: f64, c: f64, d: f64, tx: f64, ty: f64) {
        let t = Transform { a, b, c, d, tx, ty };
        self.st.transform = self.st.transform.then(&t);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn replace_transform(&mut self, a: f64, b: f64, c: f64, d: f64, tx: f64, ty: f64) {
        self.st.transform = Transform { a, b, c, d, tx, ty };
    }

    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64) {
        self.st.transform.apply(x, y)
    }

    pub fn inverse_transform_point(&self, x: f64, y: f64) -> Result<(f64, f64), String> {
        self.st
            .transform
            .inverse()
            .map(|inv| inv.apply(x, y))
            .ok_or_else(|| {
                "Graphics.inverseTransformPoint: the current transform is not invertible \
                 (a scale factor is zero)"
                    .to_string()
            })
    }
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

impl Engine {
    pub fn clear(&mut self, color: Option<(f64, f64, f64, f64)>) {
        let c = narrow(match color {
            Some((r, g, b, a)) => [r, g, b, a],
            None => self.background,
        });
        let (w, h) = self.target_size();
        let clip = match self.st.scissor {
            Some(s) => s.intersect(&Rect::surface(w, h)),
            None => Rect::surface(w, h),
        };
        self.target_mut().clear(c, clip);
    }

    /// The shared tail of every shape call: transform the path into device
    /// space, then either fill it or expand it into a stroke.
    pub(crate) fn draw_path(
        &mut self,
        mode: &str,
        local: &[Point],
        closed: bool,
    ) -> Result<(), String> {
        let t = self.st.transform;
        let device: Vec<Point> = local.iter().map(|&(x, y)| t.apply(x, y)).collect();

        let paths = match mode {
            "fill" => vec![device],
            "line" => {
                // Line width is a local-space quantity in Love2D, so it scales
                // with the transform just like the geometry does.
                let w = self.st.line_width * t.mean_scale();
                geom::stroke(&device, closed, w, self.st.line_join)
            }
            other => {
                return Err(format!(
                    "draw mode must be \"fill\" or \"line\", got `{other}`"
                ));
            }
        };

        let paint = self.paint();
        raster::fill_paths(self.target_mut(), &paths, &paint);
        Ok(())
    }

    /// Segment budget for a curve of local radius `r` under the current
    /// transform.
    pub(crate) fn segments_for(&self, r: f64) -> usize {
        geom::curve_segments(r * self.st.transform.mean_scale())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rectangle(
        &mut self,
        mode: &str,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        rx: f64,
        ry: f64,
    ) -> Result<(), String> {
        let path = if rx > 0.0 || ry > 0.0 {
            let segs = self.segments_for(rx.abs().max(ry.abs()));
            geom::rounded_rect_path(x, y, w, h, rx, ry, segs)
        } else {
            geom::rect_path(x, y, w, h)
        };
        self.draw_path(mode, &path, true)
    }

    pub fn circle(
        &mut self,
        mode: &str,
        x: f64,
        y: f64,
        radius: f64,
        segments: Option<i64>,
    ) -> Result<(), String> {
        self.ellipse(mode, x, y, radius, radius, segments)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(
        &mut self,
        mode: &str,
        x: f64,
        y: f64,
        rx: f64,
        ry: f64,
        segments: Option<i64>,
    ) -> Result<(), String> {
        let segs = match segments {
            Some(n) if n >= 3 => n as usize,
            Some(n) => return Err(format!("segment count must be at least 3, got {n}")),
            None => self.segments_for(rx.abs().max(ry.abs())),
        };
        let path = geom::ellipse_path(x, y, rx, ry, segs);
        self.draw_path(mode, &path, true)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn arc(
        &mut self,
        mode: &str,
        x: f64,
        y: f64,
        radius: f64,
        angle1: f64,
        angle2: f64,
        arctype: &str,
    ) -> Result<(), String> {
        let kind = ArcType::parse(arctype)?;
        let segs = self.segments_for(radius);
        let (path, closed) = geom::arc_path(x, y, radius, angle1, angle2, segs, kind);
        self.draw_path(mode, &path, closed)
    }

    pub fn polygon(&mut self, mode: &str, points: &[Point]) -> Result<(), String> {
        if points.len() < 3 {
            return Err("need at least 3 points".into());
        }
        self.draw_path(mode, points, true)
    }

    pub fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) -> Result<(), String> {
        self.draw_path("line", &[(x1, y1), (x2, y2)], false)
    }

    pub fn polyline(&mut self, points: &[Point]) -> Result<(), String> {
        if points.len() < 2 {
            return Err("need at least 2 points".into());
        }
        self.draw_path("line", points, false)
    }

    /// Draw one-pixel points. Each lands on the device pixel containing the
    /// transformed position, so points stay crisp under any transform.
    pub fn points(&mut self, points: &[Point]) {
        let t = self.st.transform;
        let paths: Vec<Vec<Point>> = points
            .iter()
            .map(|&(x, y)| {
                let (dx, dy) = t.apply(x, y);
                let (px, py) = (dx.floor(), dy.floor());
                vec![
                    (px, py),
                    (px + 1.0, py),
                    (px + 1.0, py + 1.0),
                    (px, py + 1.0),
                ]
            })
            .collect();
        let paint = self.paint();
        raster::fill_paths(self.target_mut(), &paths, &paint);
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// Blit one laid-out line. `y` is the line's *top*, matching how Love2D's
/// `print` anchors text.
pub(crate) fn draw_line(
    surf: &mut Surface,
    font: &mut FontRes,
    text: &str,
    x: f64,
    y: f64,
    xform: &Transform,
    paint: &Paint,
) {
    if text.is_empty() {
        return;
    }
    let baseline = y + font.ascent();
    let (glyphs, _) = font.layout(text);
    for (ch, pen) in glyphs {
        let Some(glyph) = font.glyph(ch) else {
            continue;
        };
        if glyph.mask.w == 0 || glyph.mask.h == 0 {
            continue; // whitespace carries advance but no pixels
        }
        let placement = Transform::translation(x + pen + glyph.left, baseline + glyph.top);
        raster::blit_mask(surf, &glyph.mask, &xform.then(&placement), paint);
    }
}
