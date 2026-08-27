//! Graphics module — the `Graphics.*` surface exposed to Saule.
//!
//! This is a deliberately trimmed take on Love2D's `love.graphics`: the parts
//! that matter for building user interfaces, minus the 3D pipeline, meshes,
//! particles, and video. Each function is a plain, safe Rust function annotated
//! with `#[saule_export]`; the SDK generates the C-ABI shim and the manifest
//! entry. Geometry is rendered immediately into the current render target owned
//! by [`crate::state`]; `Graphics.present` pushes the screen to the window.
//!
//! ## Differences from Love2D worth knowing
//!
//! - Drawables are **integer handles**, not objects: `newCanvas` and `newFont`
//!   return a handle you pass back to `draw`, `setCanvas`, and `setFont`.
//! - `newFont(size, path?)` takes the size first, so `newFont(24)` gives you
//!   the system UI face at 24px with no file to ship.
//! - Point lists (`polygon`, `polyline`, `points`) take a flat table of
//!   coordinates — `{x1, y1, x2, y2, ...}` — instead of varargs.
//! - Scissor rectangles are transformed by the current transform, so clipping
//!   composes with `translate` the way nested scroll views need.
//! - Shaders are not implemented: there is no GPU here, and the rasterizer is
//!   pure software. Images are PNG-only and decoded on the CPU.

use saule_sdk::prelude::*;
use saule_sdk::saule_export;

use crate::geom::Point;
use crate::raster::{Gradient, GradientKind, Rect, Stop};
use crate::state;

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

/// Accept either Saule numeric type where a coordinate is expected — a table
/// literal like `{0, 0, 10, 20}` arrives as integers.
fn number(v: &SValue) -> Option<f64> {
    v.as_float().or_else(|| v.as_int().map(|i| i as f64))
}

/// Decode a flat `{x1, y1, x2, y2, ...}` coordinate table into points.
fn points_from(table: &STable, func: &str) -> Result<Vec<Point>, String> {
    let values = table.to_vec()?;
    if values.len() % 2 != 0 {
        return Err(format!(
            "{func}: coordinate table must hold x/y pairs, got {} value(s)",
            values.len()
        ));
    }
    let mut out = Vec::with_capacity(values.len() / 2);
    for (i, pair) in values.chunks_exact(2).enumerate() {
        let (Some(x), Some(y)) = (number(&pair[0]), number(&pair[1])) else {
            return Err(format!(
                "{func}: coordinate table must contain only numbers (point {} is not)",
                i + 1
            ));
        };
        out.push((x, y));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Frame lifecycle
// ---------------------------------------------------------------------------

/// `Graphics.clear([r, g, b, a])` — begin a frame by clearing the current
/// render target. With no arguments it clears to the background colour.
#[saule_export(class = "Graphics", name = "clear")]
fn graphics_clear(
    r: Option<f64>,
    g: Option<f64>,
    b: Option<f64>,
    a: Option<f64>,
) -> Result<(), String> {
    let color = match (r, g, b) {
        (Some(r), Some(g), Some(b)) => Some((r, g, b, a.unwrap_or(1.0))),
        (None, None, None) => None,
        _ => return Err("Graphics.clear: pass all of r, g, b — or none at all".into()),
    };
    state::draw(|gfx| gfx.clear(color))?;
    Ok(())
}

/// `Graphics.present()` — end a frame: push the framebuffer to the window, pump
/// events, and apply the 60 FPS frame limit. Call at the bottom of each loop
/// iteration.
#[saule_export(class = "Graphics", name = "present")]
fn graphics_present() -> Result<(), String> {
    state::with(|e| e.present())??;
    Ok(())
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

/// `Graphics.rectangle(mode, x, y, w, h [, rx, ry])` — panels, buttons, inputs
/// and cards. Passing `rx` rounds the corners; `ry` defaults to `rx`.
#[saule_export(class = "Graphics", name = "rectangle")]
fn graphics_rectangle(
    mode: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    rx: Option<f64>,
    ry: Option<f64>,
) -> Result<(), String> {
    let rx = rx.unwrap_or(0.0);
    let ry = ry.unwrap_or(rx);
    state::draw(|gfx| gfx.rectangle(&mode, x, y, w, h, rx, ry))?
        .map_err(|e| format!("Graphics.rectangle: {e}"))
}

/// `Graphics.circle(mode, x, y, radius [, segments])` — avatars, radio dots,
/// badges. Segment count is derived from the on-screen radius when omitted.
#[saule_export(class = "Graphics", name = "circle")]
pub(crate) fn graphics_circle(
    mode: String,
    x: f64,
    y: f64,
    radius: f64,
    segments: Option<i64>,
) -> Result<(), String> {
    state::draw(|gfx| gfx.circle(&mode, x, y, radius, segments))?
        .map_err(|e| format!("Graphics.circle: {e}"))
}

/// `Graphics.ellipse(mode, x, y, radiusx, radiusy [, segments])`.
#[saule_export(class = "Graphics", name = "ellipse")]
fn graphics_ellipse(
    mode: String,
    x: f64,
    y: f64,
    radiusx: f64,
    radiusy: f64,
    segments: Option<i64>,
) -> Result<(), String> {
    state::draw(|gfx| gfx.ellipse(&mode, x, y, radiusx, radiusy, segments))?
        .map_err(|e| format!("Graphics.ellipse: {e}"))
}

/// `Graphics.arc(mode, x, y, radius, angle1, angle2 [, arctype])` — progress
/// rings and spinners. `arctype` is `"pie"` (default), `"open"`, or `"closed"`;
/// an `"open"` arc drawn in `"line"` mode with a wide line is the usual
/// progress-ring recipe.
#[saule_export(class = "Graphics", name = "arc")]
fn graphics_arc(
    mode: String,
    x: f64,
    y: f64,
    radius: f64,
    angle1: f64,
    angle2: f64,
    arctype: Option<String>,
) -> Result<(), String> {
    let arctype = arctype.unwrap_or_else(|| "pie".to_string());
    state::draw(|gfx| gfx.arc(&mode, x, y, radius, angle1, angle2, &arctype))?
        .map_err(|e| format!("Graphics.arc: {e}"))
}

/// `Graphics.polygon(mode, {x1, y1, x2, y2, ...})` — arrows, chevrons, and
/// custom shapes.
#[saule_export(class = "Graphics", name = "polygon")]
fn graphics_polygon(mode: String, points: STable) -> Result<(), String> {
    let pts = points_from(&points, "Graphics.polygon")?;
    state::draw(|gfx| gfx.polygon(&mode, &pts))?.map_err(|e| format!("Graphics.polygon: {e}"))
}

/// `Graphics.line(x1, y1, x2, y2)` — dividers, borders, connectors.
#[saule_export(class = "Graphics", name = "line")]
fn graphics_line(x1: f64, y1: f64, x2: f64, y2: f64) -> Result<(), String> {
    state::draw(|gfx| gfx.line(x1, y1, x2, y2))?.map_err(|e| format!("Graphics.line: {e}"))
}

/// `Graphics.polyline({x1, y1, x2, y2, ...})` — a multi-segment line with
/// proper joins.
#[saule_export(class = "Graphics", name = "polyline")]
fn graphics_polyline(points: STable) -> Result<(), String> {
    let pts = points_from(&points, "Graphics.polyline")?;
    state::draw(|gfx| gfx.polyline(&pts))?.map_err(|e| format!("Graphics.polyline: {e}"))
}

/// `Graphics.points({x1, y1, x2, y2, ...})` — one pixel per pair.
#[saule_export(class = "Graphics", name = "points")]
fn graphics_points(points: STable) -> Result<(), String> {
    let pts = points_from(&points, "Graphics.points")?;
    state::draw(|gfx| gfx.points(&pts))?;
    Ok(())
}

/// `Graphics.point(x, y)` — a single pixel.
#[saule_export(class = "Graphics", name = "point")]
fn graphics_point(x: f64, y: f64) -> Result<(), String> {
    state::draw(|gfx| gfx.points(&[(x, y)]))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// `Graphics.print(text, x, y)` — labels and static text, anchored at the
/// line's top-left. Embedded newlines start a new line.
#[saule_export(class = "Graphics", name = "print")]
fn graphics_print(text: String, x: f64, y: f64) -> Result<(), String> {
    state::draw(|gfx| gfx.print(&text, x, y))?.map_err(|e| format!("Graphics.print: {e}"))
}

/// `Graphics.printf(text, x, y, limit [, align])` — paragraphs with word wrap.
/// `align` is `"left"` (default), `"center"`, `"right"`, or `"justify"`.
#[saule_export(class = "Graphics", name = "printf")]
fn graphics_printf(
    text: String,
    x: f64,
    y: f64,
    limit: f64,
    align: Option<String>,
) -> Result<(), String> {
    let align = align.unwrap_or_else(|| "left".to_string());
    state::draw(|gfx| gfx.printf(&text, x, y, limit, &align))?
        .map_err(|e| format!("Graphics.printf: {e}"))
}

/// `Graphics.newFont(size [, path])` — load a TrueType face at a point size.
/// Without a path you get the host's UI typeface, so text works with no assets
/// to ship. Returns a font handle for `setFont`.
#[saule_export(class = "Graphics", name = "newFont")]
fn graphics_new_font(size: f64, path: Option<String>) -> Result<i64, String> {
    state::draw(|gfx| gfx.new_font(size, path.as_deref()))?
        .map_err(|e| format!("Graphics.newFont: {e}"))
}

/// `Graphics.setNewFont(size [, path])` — `newFont` followed by `setFont`.
#[saule_export(class = "Graphics", name = "setNewFont")]
fn graphics_set_new_font(size: f64, path: Option<String>) -> Result<i64, String> {
    state::draw(|gfx| -> Result<i64, String> {
        let handle = gfx.new_font(size, path.as_deref())?;
        gfx.set_font(handle)?;
        Ok(handle)
    })?
    .map_err(|e| format!("Graphics.setNewFont: {e}"))
}

/// `Graphics.setFont(handle)` — select a loaded font. Handle `0` is the
/// default face.
#[saule_export(class = "Graphics", name = "setFont")]
fn graphics_set_font(handle: i64) -> Result<(), String> {
    state::draw(|gfx| gfx.set_font(handle))??;
    Ok(())
}

/// `Graphics.getFont()` — the active font handle.
#[saule_export(class = "Graphics", name = "getFont")]
fn graphics_get_font() -> Result<i64, String> {
    state::draw(|gfx| gfx.get_font())
}

/// `Graphics.getFontHeight()` — baseline-to-baseline line height, the vertical
/// step for stacking labels.
#[saule_export(class = "Graphics", name = "getFontHeight")]
fn graphics_get_font_height() -> Result<f64, String> {
    state::draw(|gfx| gfx.font_height())?.map_err(|e| format!("Graphics.getFontHeight: {e}"))
}

/// `Graphics.getTextWidth(text)` — advance width in pixels. The measurement
/// half of layout: use it to size buttons around their labels.
#[saule_export(class = "Graphics", name = "getTextWidth")]
fn graphics_get_text_width(text: String) -> Result<f64, String> {
    state::draw(|gfx| gfx.text_width(&text))?.map_err(|e| format!("Graphics.getTextWidth: {e}"))
}

// ---------------------------------------------------------------------------
// Colour, lines, blending
// ---------------------------------------------------------------------------

/// `Graphics.setColor(r, g, b [, a])` — the colour for subsequent draws.
/// Channels are `0.0..=1.0`; alpha defaults to opaque.
#[saule_export(class = "Graphics", name = "setColor")]
fn graphics_set_color(r: f64, g: f64, b: f64, a: Option<f64>) -> Result<(), String> {
    state::draw(|gfx| gfx.set_color(r, g, b, a.unwrap_or(1.0)))?;
    Ok(())
}

/// `Graphics.getColor()` — `local r, g, b, a = Graphics.getColor()`.
#[saule_export(class = "Graphics", name = "getColor")]
fn graphics_get_color() -> Result<(f64, f64, f64, f64), String> {
    state::draw(|gfx| gfx.color())
}

/// `Graphics.setBackgroundColor(r, g, b [, a])` — the colour `clear()` uses
/// when called with no arguments.
#[saule_export(class = "Graphics", name = "setBackgroundColor")]
fn graphics_set_background_color(r: f64, g: f64, b: f64, a: Option<f64>) -> Result<(), String> {
    state::draw(|gfx| gfx.set_background_color(r, g, b, a.unwrap_or(1.0)))?;
    Ok(())
}

/// `Graphics.getBackgroundColor()`.
#[saule_export(class = "Graphics", name = "getBackgroundColor")]
fn graphics_get_background_color() -> Result<(f64, f64, f64, f64), String> {
    state::draw(|gfx| gfx.background_color())
}

/// `Graphics.setLineWidth(width)` — stroke thickness in local units, so it
/// scales with the current transform.
#[saule_export(class = "Graphics", name = "setLineWidth")]
fn graphics_set_line_width(width: f64) -> Result<(), String> {
    state::draw(|gfx| gfx.set_line_width(width))?;
    Ok(())
}

/// `Graphics.getLineWidth()`.
#[saule_export(class = "Graphics", name = "getLineWidth")]
fn graphics_get_line_width() -> Result<f64, String> {
    state::draw(|gfx| gfx.line_width())
}

/// `Graphics.setLineStyle(style)` — `"smooth"` (antialiased, the default) or
/// `"rough"` (hard pixel edges, for crisp hairlines).
#[saule_export(class = "Graphics", name = "setLineStyle")]
fn graphics_set_line_style(style: String) -> Result<(), String> {
    state::draw(|gfx| gfx.set_line_style(&style))??;
    Ok(())
}

/// `Graphics.getLineStyle()`.
#[saule_export(class = "Graphics", name = "getLineStyle")]
fn graphics_get_line_style() -> Result<String, String> {
    state::draw(|gfx| gfx.line_style().to_string())
}

/// `Graphics.setLineJoin(join)` — `"miter"` (default), `"bevel"`, or `"none"`.
#[saule_export(class = "Graphics", name = "setLineJoin")]
fn graphics_set_line_join(join: String) -> Result<(), String> {
    state::draw(|gfx| gfx.set_line_join(&join))??;
    Ok(())
}

/// `Graphics.getLineJoin()`.
#[saule_export(class = "Graphics", name = "getLineJoin")]
fn graphics_get_line_join() -> Result<String, String> {
    state::draw(|gfx| gfx.line_join().to_string())
}

/// `Graphics.setBlendMode(mode)` — `"alpha"` (default), `"add"`, `"subtract"`,
/// `"multiply"`, `"screen"`, or `"replace"`. Shadows and glows are `"alpha"`
/// and `"add"` respectively.
#[saule_export(class = "Graphics", name = "setBlendMode")]
fn graphics_set_blend_mode(mode: String) -> Result<(), String> {
    state::draw(|gfx| gfx.set_blend_mode(&mode))??;
    Ok(())
}

/// `Graphics.getBlendMode()`.
#[saule_export(class = "Graphics", name = "getBlendMode")]
fn graphics_get_blend_mode() -> Result<String, String> {
    state::draw(|gfx| gfx.blend_mode().to_string())
}

/// `Graphics.setDefaultFilter(mode)` — `"linear"` (default, smooth) or
/// `"nearest"` (crisp) sampling for scaled canvases and rotated text.
#[saule_export(class = "Graphics", name = "setDefaultFilter")]
fn graphics_set_default_filter(mode: String) -> Result<(), String> {
    state::draw(|gfx| gfx.set_default_filter(&mode))??;
    Ok(())
}

/// `Graphics.getDefaultFilter()`.
#[saule_export(class = "Graphics", name = "getDefaultFilter")]
fn graphics_get_default_filter() -> Result<String, String> {
    state::draw(|gfx| gfx.default_filter().to_string())
}

/// `Graphics.reset()` — restore colour, line settings, blend mode, filter,
/// scissor, transform, and render target to their defaults.
#[saule_export(class = "Graphics", name = "reset")]
fn graphics_reset() -> Result<(), String> {
    state::draw(|gfx| gfx.reset())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Clipping
// ---------------------------------------------------------------------------

/// `Graphics.setScissor([x, y, w, h])` — clip subsequent draws to a rectangle;
/// no arguments disables clipping. The rectangle is transformed by the current
/// transform, so it follows a translated widget.
#[saule_export(class = "Graphics", name = "setScissor")]
fn graphics_set_scissor(
    x: Option<f64>,
    y: Option<f64>,
    w: Option<f64>,
    h: Option<f64>,
) -> Result<(), String> {
    let rect = match (x, y, w, h) {
        (Some(x), Some(y), Some(w), Some(h)) => Some((x, y, w, h)),
        (None, None, None, None) => None,
        _ => return Err("Graphics.setScissor: pass all of x, y, w, h — or none at all".into()),
    };
    state::draw(|gfx| gfx.set_scissor(rect))?;
    Ok(())
}

/// `Graphics.intersectScissor(x, y, w, h)` — narrow the clip to the
/// intersection with the active one. This is what makes nested clipping
/// (a scroll view inside a scroll view) compose correctly.
#[saule_export(class = "Graphics", name = "intersectScissor")]
fn graphics_intersect_scissor(x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    state::draw(|gfx| gfx.intersect_scissor(x, y, w, h))?;
    Ok(())
}

/// `Graphics.getScissor()` — the active clip in *screen* coordinates, or the
/// full render target when clipping is off.
#[saule_export(class = "Graphics", name = "getScissor")]
fn graphics_get_scissor() -> Result<(f64, f64, f64, f64), String> {
    state::draw(|gfx| gfx.scissor())
}

// ---------------------------------------------------------------------------
// Canvases
// ---------------------------------------------------------------------------

/// `Graphics.newCanvas(width, height)` — an offscreen ARGB surface. Returns a
/// canvas handle.
#[saule_export(class = "Graphics", name = "newCanvas")]
fn graphics_new_canvas(width: i64, height: i64) -> Result<i64, String> {
    state::draw(|gfx| gfx.new_canvas(width, height))?
}

/// `Graphics.setCanvas([handle])` — route subsequent draws into a canvas; no
/// argument (or `0`) restores the screen.
#[saule_export(class = "Graphics", name = "setCanvas")]
fn graphics_set_canvas(handle: Option<i64>) -> Result<(), String> {
    state::draw(|gfx| gfx.set_canvas(handle))??;
    Ok(())
}

/// `Graphics.getCanvas()` — the bound canvas handle, or `0` for the screen.
#[saule_export(class = "Graphics", name = "getCanvas")]
fn graphics_get_canvas() -> Result<i64, String> {
    state::draw(|gfx| gfx.get_canvas())
}

/// `Graphics.draw(canvas, x, y [, angle, sx, sy, ox, oy])` — composite a canvas
/// onto the current target, tinted by the current colour.
// The arity is the language-level `Graphics.draw` signature, which mirrors
// LÖVE's; grouping the transform args into a struct here would only change
// the Rust side of a shape the Saule API already fixes.
#[allow(clippy::too_many_arguments)]
#[saule_export(class = "Graphics", name = "draw")]
fn graphics_draw(
    canvas: i64,
    x: f64,
    y: f64,
    angle: Option<f64>,
    sx: Option<f64>,
    sy: Option<f64>,
    ox: Option<f64>,
    oy: Option<f64>,
) -> Result<(), String> {
    let sx = sx.unwrap_or(1.0);
    state::draw(|gfx| {
        gfx.draw_canvas(
            canvas,
            x,
            y,
            angle.unwrap_or(0.0),
            sx,
            sy.unwrap_or(sx),
            ox.unwrap_or(0.0),
            oy.unwrap_or(0.0),
        )
    })??;
    Ok(())
}

/// `Graphics.newImage(path)` — load a PNG from disk. Returns a handle usable
/// anywhere a canvas handle is: `draw`, `drawFrame`, `imageSize`, `setCanvas`.
///
/// Decoding is not cached, so load an image once at startup rather than every
/// frame.
#[saule_export(class = "Graphics", name = "newImage")]
fn graphics_new_image(path: String) -> Result<i64, String> {
    state::draw(|gfx| gfx.new_image(&path))?
}

/// `Graphics.imageSize(handle)` — pixel dimensions of an image or canvas, as
/// `width, height`.
#[saule_export(class = "Graphics", name = "imageSize")]
fn graphics_image_size(handle: i64) -> Result<(i64, i64), String> {
    state::draw(|gfx| gfx.image_size(handle))?
}

/// `Graphics.drawFrame(image, fx, fy, fw, fh, x, y [, angle, sx, sy, ox, oy])`
/// — composite one cell of an image, for spritesheets. `fx, fy, fw, fh` is the
/// source rectangle in image pixels; the rest positions it exactly like
/// `draw`. Sampling is confined to the cell, so neighbouring frames never
/// bleed in at the edges.
#[saule_export(class = "Graphics", name = "drawFrame")]
#[allow(clippy::too_many_arguments)]
fn graphics_draw_frame(
    image: i64,
    fx: f64,
    fy: f64,
    fw: f64,
    fh: f64,
    x: f64,
    y: f64,
    angle: Option<f64>,
    sx: Option<f64>,
    sy: Option<f64>,
    ox: Option<f64>,
    oy: Option<f64>,
) -> Result<(), String> {
    let sx = sx.unwrap_or(1.0);
    state::draw(|gfx| {
        gfx.draw_frame(
            image,
            Rect::new(fx, fy, fw, fh),
            x,
            y,
            angle.unwrap_or(0.0),
            sx,
            sy.unwrap_or(sx),
            ox.unwrap_or(0.0),
            oy.unwrap_or(0.0),
        )
    })??;
    Ok(())
}

// ---------------------------------------------------------------------------
// Coordinate system
// ---------------------------------------------------------------------------

/// `Graphics.push([mode])` — save the transform. Pass `"all"` to snapshot the
/// full graphics state (colour, line settings, blend mode, scissor, font) too.
#[saule_export(class = "Graphics", name = "push")]
fn graphics_push(mode: Option<String>) -> Result<(), String> {
    let all = match mode.as_deref() {
        None | Some("transform") => false,
        Some("all") => true,
        Some(other) => {
            return Err(format!(
                "Graphics.push: unknown mode `{other}` (expected \"transform\" or \"all\")"
            ));
        }
    };
    state::draw(|gfx| gfx.push(all))?;
    Ok(())
}

/// `Graphics.pop()` — restore the matching `push`.
#[saule_export(class = "Graphics", name = "pop")]
fn graphics_pop() -> Result<(), String> {
    state::draw(|gfx| gfx.pop())??;
    Ok(())
}

/// `Graphics.origin()` — reset the transform to the identity.
#[saule_export(class = "Graphics", name = "origin")]
fn graphics_origin() -> Result<(), String> {
    state::draw(|gfx| gfx.origin())?;
    Ok(())
}

/// `Graphics.translate(dx, dy)` — positioning and scroll offsets.
#[saule_export(class = "Graphics", name = "translate")]
fn graphics_translate(dx: f64, dy: f64) -> Result<(), String> {
    state::draw(|gfx| gfx.translate(dx, dy))?;
    Ok(())
}

/// `Graphics.scale(sx [, sy])` — zoom and hover animations. `sy` defaults to
/// `sx` for uniform scaling.
#[saule_export(class = "Graphics", name = "scale")]
fn graphics_scale(sx: f64, sy: Option<f64>) -> Result<(), String> {
    state::draw(|gfx| gfx.scale(sx, sy.unwrap_or(sx)))?;
    Ok(())
}

/// `Graphics.rotate(angle)` — radians, clockwise on screen.
#[saule_export(class = "Graphics", name = "rotate")]
fn graphics_rotate(angle: f64) -> Result<(), String> {
    state::draw(|gfx| gfx.rotate(angle))?;
    Ok(())
}

/// `Graphics.shear(kx, ky)`.
#[saule_export(class = "Graphics", name = "shear")]
fn graphics_shear(kx: f64, ky: f64) -> Result<(), String> {
    state::draw(|gfx| gfx.shear(kx, ky))?;
    Ok(())
}

/// `Graphics.applyTransform(a, b, c, d, tx, ty)` — compose an affine matrix
/// onto the current transform. Columns are `(a, b)`, `(c, d)`, `(tx, ty)`.
#[saule_export(class = "Graphics", name = "applyTransform")]
fn graphics_apply_transform(
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
) -> Result<(), String> {
    state::draw(|gfx| gfx.apply_transform(a, b, c, d, tx, ty))?;
    Ok(())
}

/// `Graphics.replaceTransform(a, b, c, d, tx, ty)` — overwrite the current
/// transform outright.
#[saule_export(class = "Graphics", name = "replaceTransform")]
fn graphics_replace_transform(
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
) -> Result<(), String> {
    state::draw(|gfx| gfx.replace_transform(a, b, c, d, tx, ty))?;
    Ok(())
}

/// `Graphics.getStackDepth()` — pushes without a matching pop.
#[saule_export(class = "Graphics", name = "getStackDepth")]
fn graphics_get_stack_depth() -> Result<i64, String> {
    state::draw(|gfx| gfx.stack_depth())
}

/// `Graphics.transformPoint(x, y)` — local coordinates to screen coordinates.
#[saule_export(class = "Graphics", name = "transformPoint")]
fn graphics_transform_point(x: f64, y: f64) -> Result<(f64, f64), String> {
    state::draw(|gfx| gfx.transform_point(x, y))
}

/// `Graphics.inverseTransformPoint(x, y)` — screen coordinates back to local
/// ones. This is how you hit-test the mouse against a transformed widget.
#[saule_export(class = "Graphics", name = "inverseTransformPoint")]
fn graphics_inverse_transform_point(x: f64, y: f64) -> Result<(f64, f64), String> {
    state::draw(|gfx| gfx.inverse_transform_point(x, y))?
}

// ---------------------------------------------------------------------------
// Dimensions
// ---------------------------------------------------------------------------

/// `Graphics.getWidth()` — window width in pixels.
#[saule_export(class = "Graphics", name = "getWidth")]
fn graphics_get_width() -> Result<i64, String> {
    state::with(|e| e.size().0 as i64)
}

/// `Graphics.getHeight()` — window height in pixels.
#[saule_export(class = "Graphics", name = "getHeight")]
fn graphics_get_height() -> Result<i64, String> {
    state::with(|e| e.size().1 as i64)
}

/// `Graphics.getDimensions()` — `local w, h = Graphics.getDimensions()`.
#[saule_export(class = "Graphics", name = "getDimensions")]
fn graphics_get_dimensions() -> Result<(i64, i64), String> {
    state::with(|e| {
        let (w, h) = e.size();
        (w as i64, h as i64)
    })
}

/// `Graphics.getDPIScale()` — always `1.0`: the engine works in physical
/// pixels and does no DPI scaling of its own.
#[saule_export(class = "Graphics", name = "getDPIScale")]
fn graphics_get_dpi_scale() -> Result<f64, String> {
    state::with(|e| e.dpi_scale())
}

/// `Graphics.getPixelWidth()` — width in physical pixels.
#[saule_export(class = "Graphics", name = "getPixelWidth")]
fn graphics_get_pixel_width() -> Result<i64, String> {
    state::with(|e| e.size().0 as i64)
}

/// `Graphics.getPixelHeight()` — height in physical pixels.
#[saule_export(class = "Graphics", name = "getPixelHeight")]
fn graphics_get_pixel_height() -> Result<i64, String> {
    state::with(|e| e.size().1 as i64)
}

/// `Graphics.getPixelDimensions()` — physical pixel size as two values.
#[saule_export(class = "Graphics", name = "getPixelDimensions")]
fn graphics_get_pixel_dimensions() -> Result<(i64, i64), String> {
    state::with(|e| {
        let (w, h) = e.size();
        (w as i64, h as i64)
    })
}

// ---------------------------------------------------------------------------
// Resource lifetime
// ---------------------------------------------------------------------------

/// `Graphics.release(handle)` — free a canvas or image and its pixels.
///
/// Canvases and images used to live for the whole process: the registry only
/// ever grew, so a view that reallocated its canvas on every resize leaked the
/// old one, and dropping an image cache freed the handles but not the memory
/// behind them. Releasing is explicit rather than automatic because the engine
/// cannot see how many Saule values still hold the handle.
///
/// A released handle is not reused: a later `draw` with it reports an error
/// rather than silently addressing whatever has since taken the slot.
#[saule_export(class = "Graphics", name = "release")]
fn graphics_release(handle: i64) -> Result<(), String> {
    state::draw(|gfx| gfx.release_canvas(handle))?
}

/// `Graphics.releaseFont(handle)` — free a loaded font and its glyph cache.
///
/// Refused while the font is selected or held by a `push("all")` state, since
/// either would leave a live reference to a freed slot.
#[saule_export(class = "Graphics", name = "releaseFont")]
fn graphics_release_font(handle: i64) -> Result<(), String> {
    state::draw(|gfx| gfx.release_font(handle))?
}

/// `Graphics.getStats()` — live canvases, live fonts, and the bytes their
/// pixels occupy, as three values.
///
/// The number a leak shows up in: hold it steady across a few hundred frames
/// and the frame is not allocating anything it fails to release.
#[saule_export(class = "Graphics", name = "getStats")]
fn graphics_get_stats() -> Result<(i64, i64, i64), String> {
    state::draw(|gfx| {
        let (canvases, fonts) = gfx.live_counts();
        (canvases, fonts, gfx.canvas_bytes())
    })
}

// ---------------------------------------------------------------------------
// Fallible loading
// ---------------------------------------------------------------------------

/// `Graphics.loadImage(path)` — `newImage` that reports failure as `nil`.
///
/// A native error ends the program, so code that wants to skip a missing asset
/// had to `Io.open` the file first purely to find out whether it exists — two
/// opens, and still a race against anything that removes it in between. This
/// returns the handle or `nil`, with no probe and no race.
#[saule_export(class = "Graphics", name = "loadImage")]
fn graphics_load_image(path: String) -> Result<Option<i64>, String> {
    state::draw(|gfx| gfx.try_new_image(&path))
}

/// `Graphics.loadFont(size [, path])` — `newFont` that reports failure as
/// `nil`, so a missing typeface can fall back to the default face instead of
/// ending the program.
#[saule_export(class = "Graphics", name = "loadFont")]
fn graphics_load_font(size: f64, path: Option<String>) -> Result<Option<i64>, String> {
    state::draw(|gfx| gfx.try_new_font(size, path.as_deref()))
}

/// `Graphics.newImageFromBase64(data)` — decode a base64-encoded PNG.
///
/// The native ABI carries strings as UTF-8, so raw PNG bytes cannot cross it;
/// base64 is what lets an asset be embedded in a `.sau` source file, or arrive
/// over a network, without a temporary file on the way in.
#[saule_export(class = "Graphics", name = "newImageFromBase64")]
fn graphics_new_image_from_base64(data: String) -> Result<i64, String> {
    state::draw(|gfx| gfx.new_image_from_base64(&data))?
}

/// `Graphics.saveImage(handle, path)` — write a canvas, image, or the screen
/// (handle `0`) to `path` as a PNG.
///
/// The screenshot, and the thing that makes a rendering regression testable:
/// draw a frame, save it, compare it against a reference.
#[saule_export(class = "Graphics", name = "saveImage")]
fn graphics_save_image(handle: i64, path: String) -> Result<(), String> {
    state::draw(|gfx| gfx.save_image(handle, &path))?
}

// ---------------------------------------------------------------------------
// Gradients
// ---------------------------------------------------------------------------

/// Decode a flat `{position, r, g, b, a, ...}` stop table.
///
/// Five numbers per stop, in the same flat-table style the point lists use.
/// Positions run `0.0..=1.0` along the gradient's axis.
fn stops_from(table: &STable, func: &str) -> Result<Vec<Stop>, String> {
    let values = table.to_vec()?;
    if values.is_empty() || values.len() % 5 != 0 {
        return Err(format!(
            "{func}: the stop table must hold 5 numbers per stop \
             (position, r, g, b, a), got {} value(s)",
            values.len()
        ));
    }

    let mut out = Vec::with_capacity(values.len() / 5);
    for (i, stop) in values.chunks_exact(5).enumerate() {
        let mut parsed = [0.0f64; 5];
        for (slot, value) in parsed.iter_mut().zip(stop) {
            *slot = number(value).ok_or_else(|| {
                format!("{func}: stop {} contains a value that is not a number", i + 1)
            })?;
        }
        out.push(Stop {
            at: parsed[0] as f32,
            color: [
                parsed[1] as f32,
                parsed[2] as f32,
                parsed[3] as f32,
                parsed[4] as f32,
            ],
        });
    }
    Ok(out)
}

/// `Graphics.setLinearGradient(x0, y0, x1, y1, stops)` — fill shapes with a
/// ramp along the axis from `(x0, y0)` to `(x1, y1)` instead of a flat colour.
///
/// The coordinates are local and baked through the current transform when the
/// gradient is set, so it stays anchored to the shape under a later
/// `translate` — the same rule scissors follow. It applies to fills and
/// strokes; images and text keep using the flat colour as their tint.
#[saule_export(class = "Graphics", name = "setLinearGradient")]
fn graphics_set_linear_gradient(
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    stops: STable,
) -> Result<(), String> {
    let stops = stops_from(&stops, "Graphics.setLinearGradient")?;
    let gradient = Gradient::new(GradientKind::Linear { x0, y0, x1, y1 }, &stops)
        .map_err(|e| format!("Graphics.setLinearGradient: {e}"))?;
    state::draw(|gfx| gfx.set_gradient(gradient))
}

/// `Graphics.setRadialGradient(cx, cy, radius, stops)` — a ramp running
/// outward from `(cx, cy)`.
#[saule_export(class = "Graphics", name = "setRadialGradient")]
fn graphics_set_radial_gradient(
    cx: f64,
    cy: f64,
    radius: f64,
    stops: STable,
) -> Result<(), String> {
    let stops = stops_from(&stops, "Graphics.setRadialGradient")?;
    let gradient = Gradient::new(GradientKind::Radial { cx, cy, radius }, &stops)
        .map_err(|e| format!("Graphics.setRadialGradient: {e}"))?;
    state::draw(|gfx| gfx.set_gradient(gradient))
}

/// `Graphics.clearGradient()` — go back to filling with the flat colour.
#[saule_export(class = "Graphics", name = "clearGradient")]
fn graphics_clear_gradient() -> Result<(), String> {
    state::draw(|gfx| gfx.clear_gradient())
}

/// `Graphics.hasGradient()` — whether a gradient is currently the fill source.
#[saule_export(class = "Graphics", name = "hasGradient")]
fn graphics_has_gradient() -> Result<bool, String> {
    state::draw(|gfx| gfx.has_gradient())
}
