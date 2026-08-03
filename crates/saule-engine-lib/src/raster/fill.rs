//! Scanline path filling: edge accumulation, span coverage merging,
//! and the axis-aligned fast path.

use crate::geom::Point;

use super::*;

/// One edge of the flattened polygon set, kept in "top to bottom" form with
/// its original direction recorded for the nonzero winding count.
pub(crate) struct Edge {
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
pub(crate) fn blend_run(
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
pub(crate) fn axis_aligned_rect(paths: &[Vec<Point>]) -> Option<Rect> {
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
pub(crate) fn fill_axis_aligned_rect(surf: &mut Surface, rect: Rect, paint: &Paint, clip: Rect) {
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
pub(crate) fn merge_spans(spans: &mut Vec<(usize, usize)>) {
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
pub(crate) fn cov8(c: f32) -> u32 {
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
pub(crate) fn add_span(
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
