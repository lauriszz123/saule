//! Headless renderer tests.
//!
//! These are what the [`Renderer`] split bought. The drawing pipeline used to
//! be reachable only through a live `minifb` window, so transforms, scissor
//! composition, canvas targeting and resource lifetime had no coverage at all —
//! the rasterizer was well tested, and everything that drove it was not.

use super::*;
use crate::raster::{BlendMode, Gradient, GradientKind, Stop};

/// A renderer over a `w` × `h` screen, cleared to transparent so a written
/// pixel is unambiguous.
fn renderer(w: usize, h: usize) -> Renderer {
    let mut r = Renderer::headless(w, h);
    r.screen = crate::raster::Surface::new(w, h);
    r
}

impl Renderer {
    fn pixel(&self, x: usize, y: usize) -> u32 {
        self.screen.buf[y * self.screen.w + x]
    }

    fn canvas_pixel(&self, handle: i64, x: usize, y: usize) -> u32 {
        let idx = self.canvas_index(handle, "test").expect("live handle");
        let surface = self.canvases[idx].value.as_ref().expect("resident");
        surface.buf[y * surface.w + x]
    }

    /// Every pixel that is not fully transparent.
    fn written(&self) -> usize {
        self.screen.buf.iter().filter(|&&p| p >> 24 != 0).count()
    }
}

fn opaque_red(r: &mut Renderer) {
    r.set_color(1.0, 0.0, 0.0, 1.0);
}

// ---------------------------------------------------------------------------
// Graphics state
// ---------------------------------------------------------------------------

#[test]
fn default_state_matches_love_defaults() {
    let s = GState::default();
    assert_eq!(s.color, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(s.line_width, 1.0);
    assert_eq!(s.line_join, crate::geom::LineJoin::Miter);
    assert_eq!(s.blend, BlendMode::Alpha);
    assert!(s.smooth);
    assert!(s.scissor.is_none());
    assert_eq!(s.transform, crate::geom::Transform::IDENTITY);
    assert_eq!(s.font, 0);
    assert!(s.gradient.is_none());
}

#[test]
fn a_bare_push_saves_only_the_transform() {
    let mut r = renderer(4, 4);
    r.push(false);
    r.translate(10.0, 0.0);
    r.set_color(0.0, 1.0, 0.0, 1.0);
    r.pop().expect("pop");

    assert_eq!(r.st.transform, crate::geom::Transform::IDENTITY);
    // The colour was not part of the saved frame, so it survives the pop.
    assert_eq!(r.color(), (0.0, 1.0, 0.0, 1.0));
}

#[test]
fn push_all_saves_the_whole_state() {
    let mut r = renderer(4, 4);
    r.push(true);
    r.set_color(0.0, 1.0, 0.0, 1.0);
    r.set_line_width(9.0);
    r.pop().expect("pop");

    assert_eq!(r.color(), (1.0, 1.0, 1.0, 1.0));
    assert_eq!(r.line_width(), 1.0);
}

#[test]
fn popping_an_empty_stack_is_an_error_rather_than_a_panic() {
    let mut r = renderer(4, 4);
    assert!(r.pop().is_err());
}

// ---------------------------------------------------------------------------
// Transforms and clipping
// ---------------------------------------------------------------------------

#[test]
fn a_translate_moves_what_is_drawn() {
    let mut r = renderer(8, 8);
    opaque_red(&mut r);
    r.translate(4.0, 4.0);
    r.rectangle("fill", 0.0, 0.0, 2.0, 2.0, 0.0, 0.0).expect("fill");

    assert_eq!(r.pixel(0, 0) >> 24, 0, "the origin must stay untouched");
    assert_eq!(r.pixel(4, 4), 0xFFFF_0000);
    assert_eq!(r.pixel(5, 5), 0xFFFF_0000);
}

#[test]
fn a_scissor_is_transformed_with_the_geometry() {
    // The property a scroll view depends on: clip and content move together,
    // so a clipped child stays clipped when its parent is translated.
    let mut r = renderer(8, 8);
    opaque_red(&mut r);
    r.translate(4.0, 0.0);
    r.set_scissor(Some((0.0, 0.0, 2.0, 8.0)));
    r.rectangle("fill", 0.0, 0.0, 8.0, 8.0, 0.0, 0.0).expect("fill");

    // The clip landed at device x = 4..6, not 0..2.
    assert_eq!(r.pixel(0, 0) >> 24, 0);
    assert_eq!(r.pixel(4, 0), 0xFFFF_0000);
    assert_eq!(r.pixel(5, 0), 0xFFFF_0000);
    assert_eq!(r.pixel(6, 0) >> 24, 0);
}

#[test]
fn intersect_scissor_narrows_an_existing_clip() {
    let mut r = renderer(8, 8);
    opaque_red(&mut r);
    r.set_scissor(Some((0.0, 0.0, 6.0, 8.0)));
    r.intersect_scissor(2.0, 0.0, 6.0, 8.0);
    r.rectangle("fill", 0.0, 0.0, 8.0, 8.0, 0.0, 0.0).expect("fill");

    // Only the overlap, 2..6, survives.
    assert_eq!(r.pixel(1, 0) >> 24, 0);
    assert_eq!(r.pixel(2, 0), 0xFFFF_0000);
    assert_eq!(r.pixel(5, 0), 0xFFFF_0000);
    assert_eq!(r.pixel(6, 0) >> 24, 0);
}

#[test]
fn get_scissor_reports_device_coordinates() {
    let mut r = renderer(8, 8);
    r.translate(3.0, 1.0);
    r.set_scissor(Some((0.0, 0.0, 2.0, 2.0)));
    assert_eq!(r.scissor(), (3.0, 1.0, 2.0, 2.0));
}

#[test]
fn a_resize_drops_a_clip_that_would_outlive_its_surface() {
    let mut r = renderer(64, 64);
    r.set_scissor(Some((40.0, 40.0, 10.0, 10.0)));
    r.resize_screen(8, 8);

    // Kept, that clip sits entirely outside the new surface and every
    // following frame would silently draw nothing.
    assert!(r.st.scissor.is_none());
    opaque_red(&mut r);
    r.rectangle("fill", 0.0, 0.0, 8.0, 8.0, 0.0, 0.0).expect("fill");
    assert_eq!(r.written(), 64);
}

// ---------------------------------------------------------------------------
// Render targets
// ---------------------------------------------------------------------------

#[test]
fn drawing_into_a_canvas_leaves_the_screen_alone() {
    let mut r = renderer(8, 8);
    let canvas = r.new_canvas(4, 4).expect("allocate");

    r.set_canvas(Some(canvas)).expect("bind");
    opaque_red(&mut r);
    r.rectangle("fill", 0.0, 0.0, 4.0, 4.0, 0.0, 0.0).expect("fill");
    r.set_canvas(None).expect("unbind");

    assert_eq!(r.written(), 0, "the screen must be untouched");
    assert_eq!(r.canvas_pixel(canvas, 0, 0), 0xFFFF_0000);
}

#[test]
fn a_canvas_composites_back_onto_the_screen() {
    let mut r = renderer(8, 8);
    let canvas = r.new_canvas(2, 2).expect("allocate");

    r.set_canvas(Some(canvas)).expect("bind");
    opaque_red(&mut r);
    r.rectangle("fill", 0.0, 0.0, 2.0, 2.0, 0.0, 0.0).expect("fill");
    r.set_canvas(None).expect("unbind");

    r.set_color(1.0, 1.0, 1.0, 1.0);
    r.draw_canvas(canvas, 3.0, 3.0, 0.0, 1.0, 1.0, 0.0, 0.0)
        .expect("draw");

    assert_eq!(r.pixel(3, 3), 0xFFFF_0000);
    assert_eq!(r.pixel(2, 2) >> 24, 0);
}

#[test]
fn a_canvas_cannot_be_drawn_onto_itself() {
    let mut r = renderer(8, 8);
    let canvas = r.new_canvas(4, 4).expect("allocate");
    r.set_canvas(Some(canvas)).expect("bind");

    let err = r
        .draw_canvas(canvas, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0)
        .expect_err("self-draw must be refused");
    assert!(err.contains("itself"), "got: {err}");
}

#[test]
fn get_canvas_round_trips_the_handle_it_was_given() {
    let mut r = renderer(8, 8);
    let canvas = r.new_canvas(2, 2).expect("allocate");

    assert_eq!(r.get_canvas(), 0, "the screen reports handle 0");
    r.set_canvas(Some(canvas)).expect("bind");
    assert_eq!(r.get_canvas(), canvas);
}

// ---------------------------------------------------------------------------
// Resource lifetime
// ---------------------------------------------------------------------------

#[test]
fn releasing_a_canvas_frees_its_pixels() {
    let mut r = renderer(8, 8);
    let canvas = r.new_canvas(64, 64).expect("allocate");
    assert_eq!(r.live_counts().0, 1);
    assert_eq!(r.canvas_bytes(), 64 * 64 * 4);

    r.release_canvas(canvas).expect("release");
    assert_eq!(r.live_counts().0, 0);
    assert_eq!(r.canvas_bytes(), 0);
}

#[test]
fn a_released_handle_is_refused_rather_than_reused() {
    let mut r = renderer(8, 8);
    let first = r.new_canvas(4, 4).expect("allocate");
    r.release_canvas(first).expect("release");

    // The slot comes back, but under a new tag — this is the whole point of
    // tagging handles. Without it the stale handle would silently address a
    // different canvas.
    let second = r.new_canvas(4, 4).expect("reallocate");
    assert_ne!(first, second);

    let err = r.canvas_index(first, "test").expect_err("stale handle");
    assert!(err.contains("released"), "got: {err}");
    assert!(r.canvas_index(second, "test").is_ok());
}

#[test]
fn releasing_twice_is_an_error() {
    let mut r = renderer(8, 8);
    let canvas = r.new_canvas(4, 4).expect("allocate");
    r.release_canvas(canvas).expect("release");
    assert!(r.release_canvas(canvas).is_err());
}

#[test]
fn the_bound_target_cannot_be_released() {
    let mut r = renderer(8, 8);
    let canvas = r.new_canvas(4, 4).expect("allocate");
    r.set_canvas(Some(canvas)).expect("bind");

    let err = r.release_canvas(canvas).expect_err("must be refused");
    assert!(err.contains("render target"), "got: {err}");
}

#[test]
fn handles_never_carry_over_between_renderers() {
    // Two windows in one process: `Window.create` used to reset the registry,
    // so a handle from the old window addressed a *different* canvas in the
    // new one instead of failing.
    let mut first = renderer(8, 8);
    let stale = first.new_canvas(4, 4).expect("allocate");

    let mut second = renderer(8, 8);
    second.new_canvas(4, 4).expect("allocate");

    assert!(second.canvas_index(stale, "test").is_err());
}

#[test]
fn a_malformed_handle_is_rejected() {
    let r = renderer(8, 8);
    for handle in [-1, 0, 7, i64::MAX] {
        assert!(r.canvas_index(handle, "test").is_err(), "accepted {handle}");
    }
}

#[test]
fn the_default_font_can_never_be_released() {
    let mut r = renderer(8, 8);
    let err = r.release_font(0).expect_err("must be refused");
    assert!(err.contains("default font"), "got: {err}");
}

#[test]
fn stats_count_the_default_font_as_no_allocation() {
    let r = renderer(8, 8);
    assert_eq!(r.live_counts(), (0, 0));
}

// ---------------------------------------------------------------------------
// Scratch reuse
// ---------------------------------------------------------------------------

#[test]
fn a_reused_scratch_draws_the_same_pixels_as_a_fresh_one() {
    // The correctness condition for making the renderer allocation-free: the
    // coverage buffer is now shared between draw calls, so a shape must not be
    // able to see what the previous one left in it.
    let shape = |r: &mut Renderer| {
        r.set_color(1.0, 0.0, 0.0, 1.0);
        r.circle("fill", 8.0, 8.0, 6.0, None).expect("circle");
    };

    let mut fresh = renderer(24, 24);
    shape(&mut fresh);

    // A wide shape first, so the shared buffer is larger than the next needs.
    let mut reused = renderer(24, 24);
    reused.set_color(0.0, 0.0, 1.0, 1.0);
    reused
        .rectangle("fill", 0.0, 18.0, 24.0, 4.0, 0.0, 0.0)
        .expect("wide");
    reused.polygon("fill", &[(0.0, 20.0), (23.0, 20.0), (12.0, 23.0)])
        .expect("wide scanline shape");
    reused.clear(Some((0.0, 0.0, 0.0, 0.0)));
    shape(&mut reused);

    assert_eq!(reused.screen.buf, fresh.screen.buf);
}

#[test]
fn a_narrow_shape_after_a_wide_one_is_unaffected_by_it() {
    // The shared coverage buffer is sized to the widest shape so far. If a
    // later, narrower shape could see what the wide one left behind, this is
    // where it would show.
    let mut alone = renderer(32, 8);
    alone.set_color(1.0, 1.0, 1.0, 1.0);
    alone.circle("fill", 4.0, 4.0, 3.0, None).expect("circle");

    let mut after = renderer(32, 8);
    after.set_color(1.0, 1.0, 1.0, 1.0);
    after
        .polygon("fill", &[(0.0, 0.0), (31.0, 0.0), (31.0, 7.0), (0.0, 7.0)])
        .expect("wide shape through the scanline filler");
    after.clear(Some((0.0, 0.0, 0.0, 0.0)));
    after.circle("fill", 4.0, 4.0, 3.0, None).expect("circle");

    assert_eq!(after.screen.buf, alone.screen.buf);
}

// ---------------------------------------------------------------------------
// Gradients
// ---------------------------------------------------------------------------

fn black_to_white() -> Vec<Stop> {
    vec![
        Stop {
            at: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
        },
        Stop {
            at: 1.0,
            color: [1.0, 1.0, 1.0, 1.0],
        },
    ]
}

#[test]
fn a_linear_gradient_runs_along_its_axis() {
    let mut r = renderer(16, 4);
    let gradient = Gradient::new(
        GradientKind::Linear {
            x0: 0.0,
            y0: 0.0,
            x1: 16.0,
            y1: 0.0,
        },
        &black_to_white(),
    )
    .expect("gradient");

    r.set_gradient(gradient);
    r.rectangle("fill", 0.0, 0.0, 16.0, 4.0, 0.0, 0.0).expect("fill");

    let left = r.pixel(0, 2) & 0xFF;
    let middle = r.pixel(8, 2) & 0xFF;
    let right = r.pixel(15, 2) & 0xFF;
    assert!(left < middle && middle < right, "{left} {middle} {right}");
    assert!(left < 20 && right > 235, "endpoints: {left} {right}");
}

#[test]
fn a_gradient_is_constant_across_its_axis() {
    let mut r = renderer(8, 8);
    let gradient = Gradient::new(
        GradientKind::Linear {
            x0: 0.0,
            y0: 0.0,
            x1: 8.0,
            y1: 0.0,
        },
        &black_to_white(),
    )
    .expect("gradient");

    r.set_gradient(gradient);
    r.rectangle("fill", 0.0, 0.0, 8.0, 8.0, 0.0, 0.0).expect("fill");

    // Every row is the same ramp: the parameter ignores y.
    for y in 1..8 {
        for x in 0..8 {
            assert_eq!(r.pixel(x, y), r.pixel(x, 0), "row {y} differs at x={x}");
        }
    }
}

#[test]
fn a_radial_gradient_is_symmetric_about_its_centre() {
    let mut r = renderer(16, 16);
    let gradient = Gradient::new(
        GradientKind::Radial {
            cx: 8.0,
            cy: 8.0,
            radius: 8.0,
        },
        &black_to_white(),
    )
    .expect("gradient");

    r.set_gradient(gradient);
    r.rectangle("fill", 0.0, 0.0, 16.0, 16.0, 0.0, 0.0).expect("fill");

    for offset in 1..8 {
        assert_eq!(r.pixel(8 - offset, 8), r.pixel(8 + offset - 1, 8));
        assert_eq!(r.pixel(8, 8 - offset), r.pixel(8, 8 + offset - 1));
    }
}

#[test]
fn a_gradient_is_anchored_when_it_is_set() {
    // Baked through the transform at set time, so a later translate moves the
    // shape and its gradient together instead of sliding one across the other.
    let mut moved = renderer(16, 4);
    let gradient = Gradient::new(
        GradientKind::Linear {
            x0: 0.0,
            y0: 0.0,
            x1: 8.0,
            y1: 0.0,
        },
        &black_to_white(),
    )
    .expect("gradient");

    moved.translate(8.0, 0.0);
    moved.set_gradient(gradient);
    moved.rectangle("fill", 0.0, 0.0, 8.0, 4.0, 0.0, 0.0).expect("fill");

    let mut plain = renderer(16, 4);
    plain.set_gradient(gradient);
    plain.rectangle("fill", 0.0, 0.0, 8.0, 4.0, 0.0, 0.0).expect("fill");

    // The translated ramp is the untranslated one, shifted by 8 px.
    for x in 0..8 {
        assert_eq!(moved.pixel(x + 8, 2), plain.pixel(x, 2), "column {x}");
    }
}

#[test]
fn clearing_the_gradient_restores_the_flat_colour() {
    let mut r = renderer(8, 8);
    let gradient = Gradient::new(
        GradientKind::Linear {
            x0: 0.0,
            y0: 0.0,
            x1: 8.0,
            y1: 0.0,
        },
        &black_to_white(),
    )
    .expect("gradient");

    r.set_gradient(gradient);
    assert!(r.has_gradient());
    r.clear_gradient();
    assert!(!r.has_gradient());

    opaque_red(&mut r);
    r.rectangle("fill", 0.0, 0.0, 8.0, 8.0, 0.0, 0.0).expect("fill");
    assert_eq!(r.pixel(0, 0), 0xFFFF_0000);
    assert_eq!(r.pixel(7, 7), 0xFFFF_0000);
}

#[test]
fn a_gradient_needs_at_least_two_stops() {
    let one = vec![Stop {
        at: 0.0,
        color: [1.0, 1.0, 1.0, 1.0],
    }];
    assert!(
        Gradient::new(
            GradientKind::Radial {
                cx: 0.0,
                cy: 0.0,
                radius: 1.0
            },
            &one
        )
        .is_err()
    );
}

#[test]
fn stops_given_out_of_order_are_sorted() {
    let reversed = vec![
        Stop {
            at: 1.0,
            color: [1.0, 1.0, 1.0, 1.0],
        },
        Stop {
            at: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
        },
    ];
    let gradient = Gradient::new(
        GradientKind::Linear {
            x0: 0.0,
            y0: 0.0,
            x1: 10.0,
            y1: 0.0,
        },
        &reversed,
    )
    .expect("gradient");

    assert!(gradient.sample(0, 0)[0] < gradient.sample(9, 0)[0]);
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

#[test]
fn print_puts_ink_on_the_target() {
    let mut r = renderer(120, 40);
    if r.ensure_font().is_err() {
        return; // no system typeface (a bare container); nothing to assert
    }
    r.set_color(1.0, 1.0, 1.0, 1.0);
    r.print("Hg", 4.0, 4.0).expect("print");

    assert!(r.written() > 0, "expected glyph coverage");
}

#[test]
fn print_respects_the_scissor() {
    let mut r = renderer(120, 40);
    if r.ensure_font().is_err() {
        return;
    }
    r.set_color(1.0, 1.0, 1.0, 1.0);
    r.set_scissor(Some((0.0, 0.0, 0.0, 0.0)));
    r.print("Hg", 4.0, 4.0).expect("print");

    assert_eq!(r.written(), 0, "an empty clip must draw nothing");
}

#[test]
fn text_width_grows_with_the_text() {
    let mut r = renderer(8, 8);
    if r.ensure_font().is_err() {
        return;
    }
    let short = r.text_width("i").expect("measure");
    let long = r.text_width("iiiii").expect("measure");
    assert!(long > short);
}

// ---------------------------------------------------------------------------
// Allocation
// ---------------------------------------------------------------------------

/// One frame's worth of the drawing a UI actually does.
fn ui_frame(r: &mut Renderer) {
    r.clear(Some((0.1, 0.1, 0.12, 1.0)));

    for row in 0..12 {
        let y = 4.0 + row as f64 * 16.0;

        r.set_color(0.2, 0.2, 0.25, 1.0);
        r.rectangle("fill", 8.0, y, 180.0, 12.0, 3.0, 3.0)
            .expect("card");

        r.set_color(0.6, 0.6, 0.7, 1.0);
        r.set_line_width(1.0);
        r.rectangle("line", 8.0, y, 180.0, 12.0, 3.0, 3.0)
            .expect("border");

        r.circle("fill", 196.0, y + 6.0, 4.0, None).expect("dot");
        r.polyline(&[(206.0, y), (216.0, y + 6.0), (226.0, y)])
            .expect("chevron");
    }
}

#[test]
fn a_steady_frame_allocates_nothing() {
    // The reason the scratch buffers exist. Every shape used to allocate a
    // device-space path, a set of stroke outlines, and — inside the filler — a
    // coverage row as wide as its bounding box, zeroed each time.
    let mut r = renderer(256, 200);

    // The first frame grows the buffers, which is where the allocations went.
    let first = crate::counting_allocator::count(|| ui_frame(&mut r));
    assert!(first > 0, "the first frame should still be growing buffers");

    // Every frame after reuses them.
    let steady = crate::counting_allocator::count(|| ui_frame(&mut r));
    assert_eq!(steady, 0, "a steady frame allocated {steady} time(s)");
}

#[test]
fn steady_text_allocates_nothing() {
    let mut r = renderer(400, 120);
    if r.ensure_font().is_err() {
        return;
    }

    let paragraph = "The quick brown fox jumps over the lazy dog, \
                     and keeps on jumping until the line has to wrap.";

    let draw = |r: &mut Renderer| {
        r.print("a label", 4.0, 4.0).expect("print");
        r.printf(paragraph, 4.0, 24.0, 300.0, "left").expect("printf");
        r.text_width("a label").expect("measure");
    };

    draw(&mut r); // grow the glyph cache and the wrap buffer
    draw(&mut r);

    let steady = crate::counting_allocator::count(|| draw(&mut r));
    assert_eq!(steady, 0, "steady text allocated {steady} time(s)");
}


