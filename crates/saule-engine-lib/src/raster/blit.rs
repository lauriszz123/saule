//! Copying pixels: masks, surface-to-surface blits, and the sampling
//! and bounds maths a transformed blit needs.

use crate::geom::Transform;

use super::*;

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
#[cfg(test)]
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
pub(crate) fn is_integral(r: &Rect) -> bool {
    r.x0.fract() == 0.0 && r.y0.fract() == 0.0 && r.x1.fract() == 0.0 && r.y1.fract() == 0.0
}

/// Pin a source coordinate to the half-pixel-inset interior of a `0..extent`
/// span, so a bilinear tap can't reach past the region being sampled.
pub(crate) fn clamp_inside(value: f64, extent: f64) -> f64 {
    if extent <= 1.0 {
        return extent / 2.0;
    }
    value.clamp(0.5, extent - 0.5)
}

/// Recognise a pure integer translation, the case a direct copy is valid for.
pub(crate) fn axis_aligned_offset(t: &Transform) -> Option<(i64, i64)> {
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
pub(crate) fn transformed_bounds(xform: &Transform, w: f64, h: f64) -> Rect {
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
pub(crate) fn sample_mask_nearest(mask: &Mask, x: f64, y: f64) -> f32 {
    let (xi, yi) = (x.floor() as i64, y.floor() as i64);
    if xi < 0 || yi < 0 || xi as usize >= mask.w || yi as usize >= mask.h {
        return 0.0;
    }
    mask.data[yi as usize * mask.w + xi as usize] as f32 / 255.0
}

#[inline]
pub(crate) fn sample_mask_linear(mask: &Mask, x: f64, y: f64) -> f32 {
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
    let p = |dx: f64, dy: f64| sample_mask_nearest(mask, x0 + dx + 0.5, y0 + dy + 0.5);
    let top = p(0.0, 0.0) + (p(1.0, 0.0) - p(0.0, 0.0)) * tx;
    let bot = p(0.0, 1.0) + (p(1.0, 1.0) - p(0.0, 1.0)) * tx;
    top + (bot - top) * ty
}
