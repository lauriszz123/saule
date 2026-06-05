//! Window module — real OS window lifecycle and the open/event state that
//! drives a Saule-side game loop.
//!
//! Each function is a plain, safe Rust function annotated with
//! `#[saule_export]`; the SDK generates the C-ABI shim and the manifest entry.

use saule_sdk::saule_export;

use crate::state;

/// `Window.create(width, height, title?)` — open a real OS window and allocate
/// its framebuffer. The optional title defaults to `"Saule"`.
#[saule_export(class = "Window", name = "create")]
pub(crate) fn window_create(width: i64, height: i64, title: Option<String>) -> Result<(), String> {
    let title = title.unwrap_or_else(|| "Saule".to_string());
    state::create(width, height, &title)?;
    crate::timer::reset_clock();
    Ok(())
}

/// `Window.isOpen()` — the game-loop condition. False once the user closes the
/// window or holds Escape (or if no window exists).
#[saule_export(class = "Window", name = "isOpen")]
pub(crate) fn window_is_open() -> bool {
    state::with(|e| e.is_open()).unwrap_or(false)
}

/// `Window.pollEvents()` — pump the OS event queue once per frame so `isOpen`
/// and input stay fresh at the top of the loop.
#[saule_export(class = "Window", name = "pollEvents")]
fn window_poll_events() -> Result<(), String> {
    state::with(|e| e.poll_events())?;
    Ok(())
}

/// `Window.close()` — close the window and end the loop.
#[saule_export(class = "Window", name = "close")]
fn window_close() {
    state::close();
}

/// `Window.getSize()` — the window's framebuffer dimensions as two values,
/// `width, height`. Use it as `local w, h = Window.getSize()`.
#[saule_export(class = "Window", name = "getSize")]
fn window_get_size() -> Result<(i64, i64), String> {
    let (w, h) = state::with(|e| e.size())?;
    Ok((w as i64, h as i64))
}
