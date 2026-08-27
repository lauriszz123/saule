//! Geometry: the affine transform and the path builders that turn shape calls
//! into device-space polygons for [`crate::raster`].
//!
//! Every drawing call funnels through the same two primitives — fill a set of
//! polygons, or stroke a polyline — so shapes are defined here purely as point
//! lists. Curves are flattened to line segments at a tessellation density
//! derived from the *device* radius, so a circle stays smooth after
//! `Graphics.scale`.

use std::f64::consts::TAU;

/// A point. In path builders these are local (pre-transform) coordinates; the
/// rasterizer only ever sees device coordinates.
pub type Point = (f64, f64);

// ---------------------------------------------------------------------------
// Affine transform
// ---------------------------------------------------------------------------

/// A 2D affine transform, stored as the six meaningful entries of
///
/// ```text
/// | a  c  tx |
/// | b  d  ty |
/// | 0  0  1  |
/// ```
///
/// so `apply(x, y) = (a*x + c*y + tx, b*x + d*y + ty)`. This is the same
/// column order Love2D's `Transform` reports, which keeps `applyTransform`
/// interchangeable between the two.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    pub fn translation(dx: f64, dy: f64) -> Self {
        Transform {
            tx: dx,
            ty: dy,
            ..Self::IDENTITY
        }
    }

    pub fn scaling(sx: f64, sy: f64) -> Self {
        Transform {
            a: sx,
            d: sy,
            ..Self::IDENTITY
        }
    }

    pub fn rotation(angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Transform {
            a: c,
            b: s,
            c: -s,
            d: c,
            tx: 0.0,
            ty: 0.0,
        }
    }

    pub fn shearing(kx: f64, ky: f64) -> Self {
        Transform {
            a: 1.0,
            b: ky,
            c: kx,
            d: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Map a local point into this transform's space.
    #[inline]
    pub fn apply(&self, x: f64, y: f64) -> Point {
        (
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        )
    }

    /// `self ∘ rhs` — the transform that applies `rhs` first, then `self`.
    pub fn then(&self, rhs: &Transform) -> Transform {
        Transform {
            a: self.a * rhs.a + self.c * rhs.b,
            b: self.b * rhs.a + self.d * rhs.b,
            c: self.a * rhs.c + self.c * rhs.d,
            d: self.b * rhs.c + self.d * rhs.d,
            tx: self.a * rhs.tx + self.c * rhs.ty + self.tx,
            ty: self.b * rhs.tx + self.d * rhs.ty + self.ty,
        }
    }

    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// The inverse, or `None` when the transform is singular (a zero scale).
    pub fn inverse(&self) -> Option<Transform> {
        let det = self.determinant();
        if det.abs() < 1e-12 {
            return None;
        }
        Some(Transform {
            a: self.d / det,
            b: -self.b / det,
            c: -self.c / det,
            d: self.a / det,
            tx: (self.c * self.ty - self.d * self.tx) / det,
            ty: (self.b * self.tx - self.a * self.ty) / det,
        })
    }

    /// An isotropic estimate of how much this transform magnifies lengths.
    /// Used to pick curve tessellation density and stroke widths so both track
    /// the on-screen size rather than the local one.
    pub fn mean_scale(&self) -> f64 {
        let s = self.determinant().abs().sqrt();
        if s.is_finite() && s > 1e-9 { s } else { 1e-9 }
    }
}

// ---------------------------------------------------------------------------
// Path builders — all in local coordinates, all closed unless noted
// ---------------------------------------------------------------------------

/// Segment count for a curve of on-screen radius `device_radius`. Matches
/// Love2D's heuristic (`10 * sqrt(r)`), clamped to a sane range so tiny
/// widgets stay cheap and huge ones stay smooth.
pub fn curve_segments(device_radius: f64) -> usize {
    let r = device_radius.abs().max(0.0);
    ((10.0 * r.sqrt()).ceil() as usize).clamp(8, 512)
}

#[cfg(test)]
pub fn rect_path(x: f64, y: f64, w: f64, h: f64) -> Vec<Point> {
    let mut out = Vec::new();
    rect_path_into(x, y, w, h, &mut out);
    out
}

/// [`rect_path`] into a caller-owned buffer, which it clears first.
pub fn rect_path_into(x: f64, y: f64, w: f64, h: f64, out: &mut Vec<Point>) {
    out.clear();
    out.extend_from_slice(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)]);
}

/// A rectangle with elliptical corners. `rx`/`ry` are clamped to half the
/// respective side, so passing a huge radius yields a stadium/circle rather
/// than self-intersecting garbage.
#[cfg(test)]
pub fn rounded_rect_path(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    rx: f64,
    ry: f64,
    segments: usize,
) -> Vec<Point> {
    let mut out = Vec::new();
    rounded_rect_path_into(x, y, w, h, rx, ry, segments, &mut out);
    out
}

/// [`rounded_rect_path`] into a caller-owned buffer, which it clears first.
#[allow(clippy::too_many_arguments)]
pub fn rounded_rect_path_into(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    rx: f64,
    ry: f64,
    segments: usize,
    pts: &mut Vec<Point>,
) {
    let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
    let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };

    let rx = rx.abs().min(w * 0.5);
    let ry = ry.abs().min(h * 0.5);
    if rx <= 0.0 || ry <= 0.0 {
        rect_path_into(x, y, w, h, pts);
        return;
    }

    // Quarter-arc resolution; at least two segments keeps a corner from
    // collapsing into a single chamfer.
    let n = (segments / 4).max(2);
    pts.clear();
    pts.reserve(n * 4 + 4);

    // Corner centres and the arc's start angle, walked clockwise in screen
    // space (y grows downward) so the path comes out consistently wound.
    let corners = [
        (x + w - rx, y + ry, -std::f64::consts::FRAC_PI_2), // top-right
        (x + w - rx, y + h - ry, 0.0),                      // bottom-right
        (x + rx, y + h - ry, std::f64::consts::FRAC_PI_2),  // bottom-left
        (x + rx, y + ry, std::f64::consts::PI),             // top-left
    ];
    for (cx, cy, start) in corners {
        for i in 0..=n {
            let t = start + std::f64::consts::FRAC_PI_2 * (i as f64 / n as f64);
            pts.push((cx + rx * t.cos(), cy + ry * t.sin()));
        }
    }
}

/// [`ellipse_path`] into a caller-owned buffer, which it clears first.
pub fn ellipse_path_into(
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
    segments: usize,
    out: &mut Vec<Point>,
) {
    let n = segments.max(3);
    out.clear();
    out.reserve(n);
    for i in 0..n {
        let t = TAU * (i as f64 / n as f64);
        out.push((cx + rx * t.cos(), cy + ry * t.sin()));
    }
}

/// How an arc's endpoints are joined up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcType {
    /// Endpoints connected through the centre — a pie slice.
    Pie,
    /// Just the curve, left open (stroked as a polyline).
    Open,
    /// Endpoints connected directly to each other — a circular segment.
    Closed,
}

impl ArcType {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "pie" => Ok(ArcType::Pie),
            "open" => Ok(ArcType::Open),
            "closed" => Ok(ArcType::Closed),
            other => Err(format!(
                "unknown arc type `{other}` (expected \"pie\", \"open\", or \"closed\")"
            )),
        }
    }
}

/// Build an arc's point list. Returns the points and whether the path should
/// be treated as closed when stroked.
#[cfg(test)]
pub fn arc_path(
    cx: f64,
    cy: f64,
    radius: f64,
    angle1: f64,
    angle2: f64,
    segments: usize,
    arctype: ArcType,
) -> (Vec<Point>, bool) {
    let mut out = Vec::new();
    let closed = arc_path_into(cx, cy, radius, angle1, angle2, segments, arctype, &mut out);
    (out, closed)
}

/// [`arc_path`] into a caller-owned buffer, which it clears first. Returns
/// whether the path should be treated as closed when stroked.
#[allow(clippy::too_many_arguments)]
pub fn arc_path_into(
    cx: f64,
    cy: f64,
    radius: f64,
    angle1: f64,
    angle2: f64,
    segments: usize,
    arctype: ArcType,
    pts: &mut Vec<Point>,
) -> bool {
    // Scale the segment count to the swept fraction of a full turn so a 10°
    // arc doesn't get the same budget as a full circle.
    let sweep = angle2 - angle1;
    let frac = (sweep.abs() / TAU).clamp(0.0, 1.0);
    let n = ((segments as f64 * frac).ceil() as usize).max(2);

    pts.clear();
    pts.reserve(n + 2);
    if arctype == ArcType::Pie {
        pts.push((cx, cy));
    }
    for i in 0..=n {
        let t = angle1 + sweep * (i as f64 / n as f64);
        pts.push((cx + radius * t.cos(), cy + radius * t.sin()));
    }
    arctype != ArcType::Open
}

// ---------------------------------------------------------------------------
// Stroking
// ---------------------------------------------------------------------------

/// How consecutive segments of a stroked path meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineJoin {
    Miter,
    Bevel,
    None,
}

impl LineJoin {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "miter" => Ok(LineJoin::Miter),
            "bevel" => Ok(LineJoin::Bevel),
            "none" => Ok(LineJoin::None),
            other => Err(format!(
                "unknown line join `{other}` (expected \"miter\", \"bevel\", or \"none\")"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LineJoin::Miter => "miter",
            LineJoin::Bevel => "bevel",
            LineJoin::None => "none",
        }
    }
}

/// Beyond this ratio a miter is replaced by a bevel, so a near-doubled-back
/// path doesn't fire a spike across the screen. Matches the SVG/canvas default.
const MITER_LIMIT: f64 = 10.0;

/// Twice the signed area of a polygon; its sign gives the winding direction.
fn signed_area2(pts: &[Point]) -> f64 {
    let mut acc = 0.0;
    for i in 0..pts.len() {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[(i + 1) % pts.len()];
        acc += x0 * y1 - x1 * y0;
    }
    acc
}

/// Force a polygon to positive winding.
///
/// The stroker emits one polygon per segment plus one per join, and the
/// rasterizer combines them with the nonzero rule — which unions overlapping
/// polygons only when they wind the *same* way. Normalising here is what keeps
/// a thick polyline's joins from punching holes in themselves.
fn wind_positive_in_place(pts: &mut [Point]) {
    if signed_area2(pts) < 0.0 {
        pts.reverse();
    }
}

/// A growable set of polygons that reuses its inner allocations.
///
/// [`PathSet::begin`] rewinds without freeing, so the vectors a stroke filled
/// last frame are the vectors it fills this frame. This is what keeps a steady
/// frame of drawing from allocating: the renderer keeps one of these and every
/// shape passes through it.
#[derive(Default)]
pub struct PathSet {
    bufs: Vec<Vec<Point>>,
    used: usize,
}

impl PathSet {
    /// Rewind to empty, keeping every buffer for reuse.
    pub fn begin(&mut self) {
        self.used = 0;
    }

    /// Add a polygon and hand back its (empty) buffer to fill.
    pub fn push(&mut self) -> &mut Vec<Point> {
        if self.used == self.bufs.len() {
            self.bufs.push(Vec::new());
        }
        let i = self.used;
        self.used += 1;
        self.bufs[i].clear();
        &mut self.bufs[i]
    }

    /// Add a polygon built elsewhere.
    pub fn push_slice(&mut self, points: &[Point]) {
        self.push().extend_from_slice(points);
    }

    /// The polygons added since the last [`PathSet::begin`].
    pub fn paths(&self) -> &[Vec<Point>] {
        &self.bufs[..self.used]
    }
}

/// The stroke expander's working memory.
#[derive(Default)]
pub struct StrokeScratch {
    /// The input path with consecutive duplicates removed.
    pts: Vec<Point>,
    /// Unit direction of each segment.
    dirs: Vec<Point>,
}

/// Expand a polyline into the polygon set that covers its stroke, allocating a
/// fresh result.
///
/// Convenience wrapper over [`stroke_into`] for callers with nowhere to keep
/// the scratch buffers. The renderer uses the scratch-taking form.
#[cfg(test)]
pub fn stroke(path: &[Point], closed: bool, width: f64, join: LineJoin) -> Vec<Vec<Point>> {
    let mut scratch = StrokeScratch::default();
    let mut out = PathSet::default();
    stroke_into(path, closed, width, join, &mut scratch, &mut out);
    out.paths().to_vec()
}

/// Expand a polyline into the polygon set that covers its stroke.
///
/// `width` is in device units and points are device coordinates, so a stroke
/// keeps a constant on-screen thickness regardless of the current transform —
/// the same behaviour Love2D's GPU stroking has.
pub fn stroke_into(
    path: &[Point],
    closed: bool,
    width: f64,
    join: LineJoin,
    scratch: &mut StrokeScratch,
    out: &mut PathSet,
) {
    let hw = (width * 0.5).max(0.05);
    out.begin();

    // Drop consecutive duplicates: zero-length segments have no direction, and
    // they'd otherwise produce degenerate joins.
    let pts = &mut scratch.pts;
    pts.clear();
    for &p in path {
        match pts.last() {
            Some(&q) if (p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9 => {}
            _ => pts.push(p),
        }
    }
    if closed
        && let (Some(&first), Some(&last)) = (pts.first(), pts.last())
        && (first.0 - last.0).abs() < 1e-9
        && (first.1 - last.1).abs() < 1e-9
    {
        pts.pop();
    }

    if pts.len() < 2 {
        // A degenerate path still deserves a visible dot, matching how a
        // zero-length line renders in most 2D APIs.
        if let Some(&(x, y)) = pts.first() {
            let dot = out.push();
            ellipse_path_into(x, y, hw, hw, 12, dot);
            wind_positive_in_place(dot);
        }
        return;
    }

    let n = pts.len();
    let seg_count = if closed { n } else { n - 1 };

    // Unit direction and unit left-normal of each segment.
    let dirs = &mut scratch.dirs;
    dirs.clear();
    for i in 0..seg_count {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[(i + 1) % n];
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt().max(1e-12);
        dirs.push((dx / len, dy / len));
    }

    // One quad per segment.
    for i in 0..seg_count {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[(i + 1) % n];
        let (dx, dy) = dirs[i];
        let (nx, ny) = (-dy * hw, dx * hw);
        let quad = out.push();
        quad.extend_from_slice(&[
            (x0 + nx, y0 + ny),
            (x1 + nx, y1 + ny),
            (x1 - nx, y1 - ny),
            (x0 - nx, y0 - ny),
        ]);
        wind_positive_in_place(quad);
    }

    if join == LineJoin::None {
        return;
    }

    // One join wedge per vertex where two segments actually meet: the interior
    // vertices of an open path, or every vertex of a closed one.
    let joint_count = if closed { n } else { n.saturating_sub(1) };
    let joint_start = if closed { 0 } else { 1 };
    for idx in joint_start..joint_count {
        // The segment arriving at this vertex, and the one leaving it.
        let prev = (idx + seg_count - 1) % seg_count;
        let (d0x, d0y) = dirs[prev];
        let (d1x, d1y) = dirs[idx];

        let cross = d0x * d1y - d0y * d1x;
        if cross.abs() < 1e-12 {
            continue; // collinear: the quads already meet flush
        }
        // Offset toward the *outside* of the turn — the inside is covered by
        // the overlapping quads.
        let side = if cross > 0.0 { -1.0 } else { 1.0 };
        let (n0x, n0y) = (-d0y * side, d0x * side);
        let (n1x, n1y) = (-d1y * side, d1x * side);

        let (px, py) = pts[idx];
        let a = (px + n0x * hw, py + n0y * hw);
        let b = (px + n1x * hw, py + n1y * hw);

        // The miter tip, when the turn is shallow enough to earn one.
        let mut tip = None;
        if join == LineJoin::Miter {
            let (mx, my) = (n0x + n1x, n0y + n1y);
            let len = (mx * mx + my * my).sqrt();
            // |n0 + n1| / 2 == cos(θ/2) for unit normals; the miter runs
            // hw / cos(θ/2) from the vertex.
            let cos_half = len * 0.5;
            if cos_half > 1e-6 && 1.0 / cos_half <= MITER_LIMIT {
                let miter = hw / cos_half;
                tip = Some((px + mx / len * miter, py + my / len * miter));
            }
        }

        let wedge = out.push();
        match tip {
            Some(tip) => wedge.extend_from_slice(&[(px, py), a, tip, b]),
            None => wedge.extend_from_slice(&[(px, py), a, b]),
        }
        wind_positive_in_place(wedge);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn identity_maps_points_unchanged() {
        let (x, y) = Transform::IDENTITY.apply(3.0, -7.0);
        assert!(close(x, 3.0) && close(y, -7.0));
    }

    #[test]
    fn compose_applies_right_hand_side_first() {
        // Scale by 2, then translate by 10 — the translation must not be
        // scaled, which is what distinguishes `then` from the other order.
        let t = Transform::translation(10.0, 0.0).then(&Transform::scaling(2.0, 2.0));
        let (x, y) = t.apply(1.0, 1.0);
        assert!(close(x, 12.0), "got {x}");
        assert!(close(y, 2.0), "got {y}");
    }

    #[test]
    fn inverse_round_trips() {
        let t = Transform::translation(30.0, -5.0)
            .then(&Transform::rotation(0.7))
            .then(&Transform::scaling(2.0, 3.0));
        let inv = t.inverse().expect("invertible");
        let (x, y) = t.apply(11.0, -4.0);
        let (bx, by) = inv.apply(x, y);
        assert!(close(bx, 11.0), "got {bx}");
        assert!(close(by, -4.0), "got {by}");
    }

    #[test]
    fn singular_transform_has_no_inverse() {
        assert!(Transform::scaling(0.0, 1.0).inverse().is_none());
    }

    #[test]
    fn mean_scale_tracks_uniform_scaling() {
        assert!(close(Transform::scaling(3.0, 3.0).mean_scale(), 3.0));
    }

    #[test]
    fn rounded_rect_clamps_oversized_radius() {
        // A radius past half the side must not fold the path inside out.
        let pts = rounded_rect_path(0.0, 0.0, 20.0, 20.0, 999.0, 999.0, 32);
        for &(x, y) in &pts {
            assert!((-0.001..=20.001).contains(&x), "x out of bounds: {x}");
            assert!((-0.001..=20.001).contains(&y), "y out of bounds: {y}");
        }
    }

    #[test]
    fn zero_radius_rounded_rect_is_a_plain_rect() {
        assert_eq!(
            rounded_rect_path(1.0, 2.0, 3.0, 4.0, 0.0, 0.0, 16),
            rect_path(1.0, 2.0, 3.0, 4.0)
        );
    }

    #[test]
    fn open_arc_is_not_closed_and_omits_the_centre() {
        let (pts, closed) = arc_path(0.0, 0.0, 10.0, 0.0, 1.0, 64, ArcType::Open);
        assert!(!closed);
        // First point sits on the rim, not at the centre.
        assert!(close(pts[0].0, 10.0) && close(pts[0].1, 0.0));
    }

    #[test]
    fn pie_arc_starts_at_the_centre() {
        let (pts, closed) = arc_path(5.0, 5.0, 10.0, 0.0, 1.0, 64, ArcType::Pie);
        assert!(closed);
        assert_eq!(pts[0], (5.0, 5.0));
    }

    #[test]
    fn stroked_polyline_emits_a_quad_per_segment_plus_joins() {
        let path = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        let polys = stroke(&path, false, 2.0, LineJoin::Miter);
        // 2 segments + 1 interior join
        assert_eq!(polys.len(), 3);
    }

    #[test]
    fn stroke_polygons_all_wind_the_same_way() {
        // Mixed winding would make the nonzero fill cancel overlaps into holes.
        let path = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        for poly in stroke(&path, true, 4.0, LineJoin::Miter) {
            assert!(signed_area2(&poly) >= 0.0);
        }
    }

    #[test]
    fn degenerate_path_still_marks_a_dot() {
        let polys = stroke(&[(4.0, 4.0)], false, 6.0, LineJoin::Miter);
        assert_eq!(polys.len(), 1);
        assert!(polys[0].len() > 3);
    }

    #[test]
    fn closed_stroke_drops_a_duplicated_final_point() {
        // An explicitly re-stated first point must not create a zero-length
        // segment and a bogus join.
        let path = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 0.0)];
        let polys = stroke(&path, true, 2.0, LineJoin::None);
        assert_eq!(polys.len(), 3);
    }

    #[test]
    fn curve_segments_grow_with_radius_and_stay_bounded() {
        assert_eq!(curve_segments(0.0), 8);
        assert!(curve_segments(100.0) > curve_segments(10.0));
        assert_eq!(curve_segments(1e9), 512);
    }
}
