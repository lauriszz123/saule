//! Mouse module — position, button state, per-frame press/release edges, wheel
//! movement, and the cursor image.
//!
//! Edges and wheel deltas are measured against the last `Window.pollEvents()`,
//! exactly like the keyboard's. Saule owns the loop, so Love2D's
//! `love.mousepressed` / `love.wheelmoved` callbacks become queries you make
//! once per frame.

use saule_sdk::saule_export;

use crate::state;

#[saule_export(class = "Mouse", name = "getPos")]
pub(crate) fn mouse_get_pos() -> Result<(f64, f64), String> {
    state::with(|e| e.mouse_pos())
}

/// `Mouse.isDown(button)` — `true` while the given button is held.
/// Button indices follow the Love2D convention: `1` = left, `2` = right,
/// `3` = middle. Returns `false` for unknown button indices or when no
/// window exists.
#[saule_export(class = "Mouse", name = "isDown")]
pub(crate) fn mouse_is_down(button: i64) -> bool {
    state::with(|e| e.mouse().is_down(button)).unwrap_or(false)
}

/// `Mouse.setCursor(style)` — swap the cursor image. One of `"arrow"`,
/// `"ibeam"`, `"crosshair"`, `"hand"`, `"grab"`, `"resizeleftright"`,
/// `"resizeupdown"`, `"resizeall"`.
#[saule_export(class = "Mouse", name = "setCursor")]
pub(crate) fn mouse_set_cursor(style: String) -> Result<(), String> {
    state::with(|e| e.set_cursor(&style))?
}

/// `Mouse.setVisible(visible)` — show or hide the cursor over the window.
#[saule_export(class = "Mouse", name = "setVisible")]
pub(crate) fn mouse_set_visible(visible: bool) -> Result<(), String> {
    state::with(|e| e.set_cursor_visible(visible))
}
