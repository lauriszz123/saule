//! Graphics module — drawing primitives backed by a software rasterizer.
//!
//! Each function is a plain, safe Rust function annotated with
//! `#[saule_export]`; the SDK generates the C-ABI shim and the manifest entry.
//! Geometry is rendered immediately into the framebuffer owned by
//! [`crate::state`]; `Graphics.present` pushes it to the window.

use saule_sdk::saule_export;

use crate::state;

/// `Graphics.setColor(r, g, b)` — set the colour used by subsequent `circle` /
/// `rectangle` calls. Channels are `0.0..=1.0`.
#[saule_export(class = "Graphics", name = "setColor")]
fn graphics_set_color(r: f64, g: f64, b: f64) -> Result<(), String> {
    state::with(|e| e.set_color(r, g, b))?;
    Ok(())
}

/// `Graphics.circle(mode, x, y, radius)` — draw a circle (`"fill"` or outline).
#[saule_export(class = "Graphics", name = "circle")]
pub(crate) fn graphics_circle(mode: String, x: f64, y: f64, radius: f64) -> Result<(), String> {
    state::with(|e| e.circle(&mode, x, y, radius))?;
    Ok(())
}

/// `Graphics.rectangle(mode, x, y, w, h)` — draw a rectangle (`"fill"` or
/// outline).
#[saule_export(class = "Graphics", name = "rectangle")]
fn graphics_rectangle(mode: String, x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    state::with(|e| e.rectangle(&mode, x, y, w, h))?;
    Ok(())
}

/// `Graphics.clear(r, g, b)` — begin a frame by clearing the framebuffer to the
/// given colour. Call at the top of each loop iteration.
#[saule_export(class = "Graphics", name = "clear")]
fn graphics_clear(r: f64, g: f64, b: f64) -> Result<(), String> {
    state::with(|e| e.clear(r, g, b))?;
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
