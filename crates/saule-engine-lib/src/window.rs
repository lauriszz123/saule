//! Window module — real OS window lifecycle and the open/event state that
//! drives a Saule-side game loop.
//!
//! Each function is a plain, safe Rust function annotated with
//! `#[saule_export]`; the SDK generates the C-ABI shim and the manifest entry.

use saule_sdk::saule_export;

use crate::state;

/// `Window.create(width, height, title?, resizable?)` — open a real OS window
/// and allocate its framebuffer. The title defaults to `"Saule"`, and the
/// window is resizable unless you say otherwise; the framebuffer follows the
/// window, so `getSize` reports the live dimensions.
#[saule_export(class = "Window", name = "create")]
pub(crate) fn window_create(
    width: i64,
    height: i64,
    title: Option<String>,
    resizable: Option<bool>,
) -> Result<(), String> {
    let title = title.unwrap_or_else(|| "Saule".to_string());
    state::create(width, height, &title, resizable.unwrap_or(true))?;
    crate::timer::reset_clock();
    Ok(())
}

/// `Window.setTitle(title)` — change the title bar text.
#[saule_export(class = "Window", name = "setTitle")]
fn window_set_title(title: String) -> Result<(), String> {
    state::with(|e| e.set_title(&title))
}

/// `Window.getPosition()` — the window's top-left corner in screen
/// coordinates, as `x, y`.
#[saule_export(class = "Window", name = "getPosition")]
fn window_get_position() -> Result<(i64, i64), String> {
    state::with(|e| e.position())
}

/// `Window.setPosition(x, y)` — move the window.
#[saule_export(class = "Window", name = "setPosition")]
fn window_set_position(x: i64, y: i64) -> Result<(), String> {
    state::with(|e| e.set_position(x, y))
}

/// `Window.setTopmost(topmost)` — keep the window above the others.
#[saule_export(class = "Window", name = "setTopmost")]
fn window_set_topmost(topmost: bool) -> Result<(), String> {
    state::with(|e| e.set_topmost(topmost))
}

/// `Window.isFocused()` — whether the window has keyboard focus. Worth checking
/// before running an animation nobody is looking at.
#[saule_export(class = "Window", name = "isFocused")]
fn window_is_focused() -> bool {
    state::with(|e| e.is_focused()).unwrap_or(false)
}

/// `Window.getScale()` — the display's scale factor: `1.0` at 96 DPI, `1.5` at
/// 150%, and so on. Sizes and positions everywhere else are in physical pixels,
/// so this is the number a UI multiplies by to stay the same apparent size on a
/// high-density display.
///
/// Windows reports the real value; other platforms report `1.0` for now.
#[saule_export(class = "Window", name = "getScale")]
fn window_get_scale() -> Result<f64, String> {
    state::with(|e| e.dpi_scale())
}

/// `Window.isOpen()` — the game-loop condition. False once the user closes the
/// window or holds Escape (or if no window exists).
#[saule_export(class = "Window", name = "isOpen")]
pub(crate) fn window_is_open() -> bool {
    state::with(|e| e.is_open()).unwrap_or(false)
}

/// `Window.pollEvents()` — pump the OS event queue and return everything that
/// happened since the last call.
///
/// Each entry is `[kind, payload…]`: `["keyPressed", key, shift, ctrl, alt]`,
/// `["mouseMoved", x, y, dx, dy]`, `["textInput", text]`, and so on. The
/// tagged-union shape does not survive the native ABI, so the Saule side
/// rebuilds it — `UIKit/Events.sau` decodes these into an `Event` enum with
/// tuple payloads to `match` on.
///
/// This is the primary input API. `Keyboard.isDown` and `Mouse.getPos` remain
/// for the question events cannot answer — what is held *right now* — but the
/// per-frame edge queries are gone; a press is an event.
#[saule_export(class = "Window", name = "pollEvents")]
fn window_poll_events() -> Result<saule_sdk::types::STable, String> {
    state::with(|e| {
        e.poll_events();
        crate::event::to_table(e.events())
    })?
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

/// `Window.setQuitOnEscape(enable)` — whether holding Escape ends the loop.
///
/// **Off by default, unlike Love2D.** Escape is how an application dismisses a
/// modal, closes a menu, or cancels an autocomplete — and when the engine
/// quits on it, the app has no way to decline. The `Closed` event and the
/// window's own close button cover the real close; this is here for a game
/// that genuinely wants the Love2D behaviour.
#[saule_export(class = "Window", name = "setQuitOnEscape")]
fn window_set_quit_on_escape(enable: bool) -> Result<(), String> {
    state::with(|e| e.set_quit_on_escape(enable))
}

/// `Window.getQuitOnEscape()` — whether Escape currently ends the loop.
#[saule_export(class = "Window", name = "getQuitOnEscape")]
fn window_get_quit_on_escape() -> bool {
    state::with(|e| e.quit_on_escape()).unwrap_or(false)
}

/// `Window.setTargetFPS(fps)` — cap the loop at `fps` frames per second, or
/// `0` to run unthrottled.
///
/// `Graphics.present` is the only thing pacing a Saule game loop, and the cap
/// used to be a hardcoded 60 with no way to ask for less — a mostly idle UI on
/// battery — or more, on a 120 Hz display.
#[saule_export(class = "Window", name = "setTargetFPS")]
fn window_set_target_fps(fps: i64) -> Result<(), String> {
    state::with(|e| e.set_target_fps(fps))?
}

/// `Window.getTargetFPS()` — the current cap, or `0` when uncapped.
#[saule_export(class = "Window", name = "getTargetFPS")]
fn window_get_target_fps() -> Result<i64, String> {
    state::with(|e| e.target_fps())
}
