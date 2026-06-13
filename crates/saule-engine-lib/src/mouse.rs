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
    state::with(|e| e.mouse_is_down(button)).unwrap_or(false)
}
