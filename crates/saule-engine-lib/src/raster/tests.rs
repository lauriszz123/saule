use super::*;
use crate::geom::{Point, Transform};

fn paint(color: [f32; 4], w: usize, h: usize) -> Paint {
    Paint {
        color,
        blend: BlendMode::Alpha,
        clip: Rect::surface(w, h),
        antialias: true,
        linear_filter: false,
        gradient: None,
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
