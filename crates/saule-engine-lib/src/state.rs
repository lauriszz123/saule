//! Shared engine state: the OS window, its render targets, the graphics state
//! machine (colour, line style, blend mode, scissor, transform, font), and the
//! glue that turns each `Graphics.*` call into work for [`crate::raster`].
//!
//! The interpreter is single-threaded and calls every native symbol from the
//! same thread, so the state lives in a `thread_local!`. That sidesteps the
//! `Send`/`Sync` requirements a `static` would impose (minifb's `Window` is
//! not `Send`), and matches the actual call pattern exactly.

mod draw;
mod input;
#[cfg(test)]
mod tests;
mod text;
mod window;

pub(crate) use draw::*;
pub(crate) use input::*;
pub(crate) use text::*;
pub(crate) use window::*;

use std::cell::RefCell;
use std::time::Instant;

use minifb::{Scale, Window, WindowOptions};

use crate::event::Event;
use crate::font::FontRes;
use crate::keyboard::{self, KeyState, TextCollector};
use crate::raster::Surface;

/// Live engine state, created by `Window.create`.
pub struct Engine {
    pub(crate) window: Window,
    /// The window's framebuffer. minifb reads it as `0RGB` and ignores alpha.
    pub(crate) screen: Surface,
    /// Canvas registry; a slot is `None` only while its surface is on loan to
    /// a draw call (see [`Engine::draw_canvas`]). Handle `n` is index `n - 1`.
    pub(crate) canvases: Vec<Option<Surface>>,
    /// Bound render target: `None` is the screen, `Some(i)` a canvas index.
    pub(crate) target: Option<usize>,
    /// Font registry. Slot 0 is the system default, loaded on first use.
    pub(crate) fonts: Vec<Option<FontRes>>,
    pub(crate) background: [f64; 4],
    pub(crate) st: GState,
    pub(crate) stack: Vec<Saved>,
    /// Bilinear rather than nearest sampling for transformed blits.
    pub(crate) linear_filter: bool,
    /// Per-frame keyboard edges, latched by [`Engine::poll_events`].
    pub(crate) keys: KeyState,
    /// Per-frame mouse edges and wheel, latched by [`Engine::poll_events`].
    pub(crate) mouse: MouseState,
    /// Instant the next presented frame should be released at — the single
    /// 60 FPS pacing point (see [`Engine::present`]).
    pub(crate) next_frame: Instant,
    /// OS scale factor, refreshed each frame so dragging a window between
    /// monitors of different DPI is picked up.
    pub(crate) scale: f64,
    /// This frame's events, rebuilt by every [`Engine::poll_events`].
    pub(crate) events: Vec<Event>,
    /// Previous pointer position, for the motion delta. `None` until the
    /// pointer is first seen, so the opening frame reports no phantom move.
    pub(crate) last_mouse: Option<(f64, f64)>,
    /// Previous focus state, so only changes are reported.
    pub(crate) last_focused: bool,
}

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
}

/// Create the window and framebuffer. Replaces any existing window.
pub fn create(width: i64, height: i64, title: &str, resizable: bool) -> Result<(), String> {
    declare_dpi_aware();

    if width <= 0 || height <= 0 {
        return Err("Window.create: width and height must be positive".into());
    }
    let width = width as usize;
    let height = height as usize;

    let mut window = Window::new(
        title,
        width,
        height,
        WindowOptions {
            resize: resizable,
            scale: Scale::X1,
            // Keep the framebuffer 1:1 with the window: the engine reallocates
            // it on resize rather than letting minifb stretch a stale buffer.
            scale_mode: minifb::ScaleMode::Stretch,
            ..WindowOptions::default()
        },
    )
    .map_err(|e| format!("Window.create: {e}"))?;

    // We pace the loop ourselves in `present` (a single sleep per frame), so
    // leave minifb's own limiter off — otherwise both `update` and
    // `update_with_buffer` would each block and halve the frame rate.
    window.set_target_fps(0);

    // Typed text and key edges arrive through this callback while `update`
    // pumps the queue; `Engine::collect_events` and `KeyState::sync` drain what
    // it collects.
    window.set_input_callback(Box::new(TextCollector));
    keyboard::reset_input();

    let scale = query_scale(&window);

    ENGINE.with(|cell| {
        *cell.borrow_mut() = Some(Engine {
            window,
            screen: Surface::opaque(width, height),
            canvases: Vec::new(),
            target: None,
            fonts: vec![None],
            background: [0.0, 0.0, 0.0, 1.0],
            st: GState::default(),
            stack: Vec::new(),
            linear_filter: true,
            keys: KeyState::default(),
            mouse: MouseState::default(),
            next_frame: Instant::now() + FRAME_DUR,
            scale,
            events: Vec::new(),
            last_mouse: None,
            last_focused: true,
        });
    });
    Ok(())
}

/// Tear the window down (closes it on drop).
pub fn close() {
    ENGINE.with(|cell| *cell.borrow_mut() = None);
}

/// Run `f` against the live engine, or return an error if no window exists.
pub fn with<R>(f: impl FnOnce(&mut Engine) -> R) -> Result<R, String> {
    ENGINE.with(|cell| {
        let mut guard = cell.borrow_mut();
        match guard.as_mut() {
            Some(engine) => Ok(f(engine)),
            None => Err("no window — call Window.create first".into()),
        }
    })
}

// ---------------------------------------------------------------------------
// Window, input, framing
// ---------------------------------------------------------------------------
