//! Shared engine state: the OS window, its render targets, the graphics state
//! machine (colour, line style, blend mode, scissor, transform, font), and the
//! glue that turns each `Graphics.*` call into work for [`crate::raster`].
//!
//! The interpreter is single-threaded and calls every native symbol from the
//! same thread, so the state lives in a `thread_local!`. That sidesteps the
//! `Send`/`Sync` requirements a `static` would impose (minifb's `Window` is
//! not `Send`), and matches the actual call pattern exactly.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use minifb::{CursorStyle, Key, MouseButton, MouseMode, Scale, Window, WindowOptions};

use crate::font::{self, Align, FontRes};
use crate::geom::{self, ArcType, LineJoin, Point, Transform};
use crate::keyboard::{self, KeyState, TextCollector};
use crate::raster::{self, BlendMode, Paint, Rect, Surface};

/// Ask the OS for this window's scale factor, with 1.0 meaning "96 DPI".
///
/// minifb has no DPI query, so this goes through the native window handle.
/// Only Windows is wired up; the others report 1.0 rather than guessing, which
/// at least makes the limitation visible instead of subtly wrong.
#[cfg(windows)]
fn query_scale(window: &Window) -> f64 {
    let handle = window.get_window_handle();

    if handle.is_null() {
        return 1.0;
    }

    // Safety: the handle is minifb's live HWND, and `GetDpiForWindow` only
    // reads from it.
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(handle as _) };

    if dpi == 0 {
        return 1.0;
    }

    f64::from(dpi) / 96.0
}

#[cfg(not(windows))]
fn query_scale(_window: &Window) -> f64 {
    1.0
}

/// Opt the process into per-monitor DPI awareness before any window exists.
///
/// Without this Windows lies: `GetDpiForWindow` reports 96 and the OS scales
/// the window's pixels up for us, which on a scaled display means a blurry UI
/// we cannot see the real resolution of. Declaring awareness hands us physical
/// pixels and an honest DPI, which is what the toolkit wants — it does its own
/// scaling.
#[cfg(windows)]
fn declare_dpi_aware() {
    use std::sync::Once;

    static ONCE: Once = Once::new();

    ONCE.call_once(|| {
        // Safety: no arguments to get wrong, and a failure (already set by a
        // manifest, or an older Windows) is reported by the return value we
        // deliberately ignore.
        unsafe {
            windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
    });
}

#[cfg(not(windows))]
fn declare_dpi_aware() {}

/// Target frame time for the 60 FPS cap.
const FRAME_DUR: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// The retained graphics state — everything `Graphics.push("all")` saves and
/// `Graphics.reset()` restores.
#[derive(Clone)]
struct GState {
    /// Kept at `f64` so `getColor` returns exactly what `setColor` was given;
    /// it narrows to `f32` only when a [`Paint`] is built.
    color: [f64; 4],
    line_width: f64,
    line_join: LineJoin,
    /// `false` is `setLineStyle("rough")`: hard, aliased edges.
    smooth: bool,
    blend: BlendMode,
    /// Device-space clip. `None` means "the whole render target".
    scissor: Option<Rect>,
    transform: Transform,
    /// Index into [`Engine::fonts`]; `0` is the lazily-loaded system default.
    font: usize,
}

impl Default for GState {
    fn default() -> Self {
        GState {
            color: [1.0, 1.0, 1.0, 1.0],
            line_width: 1.0,
            line_join: LineJoin::Miter,
            smooth: true,
            blend: BlendMode::Alpha,
            scissor: None,
            transform: Transform::IDENTITY,
            font: 0,
        }
    }
}

/// One `Graphics.push` frame. A bare `push()` is transform-only (matching
/// Love2D); `push("all")` snapshots the whole state.
enum Saved {
    TransformOnly(Transform),
    All(Box<GState>),
}

/// Per-frame mouse edges and wheel movement, latched by
/// [`Engine::poll_events`] the same way [`KeyState`] latches key edges.
///
/// minifb reports the wheel as a delta that is only valid for the `update`
/// that produced it, so it has to be captured at the frame boundary or it is
/// lost. Buttons are sampled the same way to give `wasPressed` / `wasReleased`
/// the same "since the last `pollEvents`" meaning the keyboard has.
#[derive(Default)]
pub struct MouseState {
    /// Left, right, middle — indices 0, 1, 2 (button numbers 1, 2, 3).
    down: [bool; 3],
    pressed: [bool; 3],
    released: [bool; 3],
    wheel: (f64, f64),
    /// Edges and wheel movement seen during a `present`. Same reason as
    /// [`crate::keyboard::KeyState`]: minifb pumps the OS queue twice a frame
    /// and each pump clears its own scroll delta, so half of them would be
    /// thrown away before Saule ever saw them.
    carried_pressed: [bool; 3],
    carried_released: [bool; 3],
    carried_wheel: (f64, f64),
}

/// One wheel notch in minifb's units: it reports `WHEEL_DELTA` (120) scaled by
/// 0.1, so a single click arrives as 12.0. Normalising to 1.0 per notch is what
/// makes "pixels per notch" mean anything on the Saule side.
const WHEEL_NOTCH: f64 = 12.0;

impl MouseState {
    fn sync(&mut self, window: &Window) {
        const BUTTONS: [MouseButton; 3] =
            [MouseButton::Left, MouseButton::Right, MouseButton::Middle];

        for (i, button) in BUTTONS.iter().enumerate() {
            let now = window.get_mouse_down(*button);
            self.pressed[i] = (now && !self.down[i]) || self.carried_pressed[i];
            self.released[i] = (!now && self.down[i]) || self.carried_released[i];
            self.down[i] = now;
        }

        let (wx, wy) = Self::read_wheel(window);
        self.wheel = (wx + self.carried_wheel.0, wy + self.carried_wheel.1);

        self.carried_pressed = [false; 3];
        self.carried_released = [false; 3];
        self.carried_wheel = (0.0, 0.0);
    }

    /// Sample the mouse after presenting, keeping the edges and wheel movement
    /// for the next [`MouseState::sync`].
    fn observe(&mut self, window: &Window) {
        const BUTTONS: [MouseButton; 3] =
            [MouseButton::Left, MouseButton::Right, MouseButton::Middle];

        for (i, button) in BUTTONS.iter().enumerate() {
            let now = window.get_mouse_down(*button);

            if now && !self.down[i] {
                self.carried_pressed[i] = true;
            }

            if !now && self.down[i] {
                self.carried_released[i] = true;
            }

            self.down[i] = now;
        }

        let (wx, wy) = Self::read_wheel(window);
        self.carried_wheel = (self.carried_wheel.0 + wx, self.carried_wheel.1 + wy);
    }

    /// The wheel delta in notches: 1.0 per click, positive away from the user.
    fn read_wheel(window: &Window) -> (f64, f64) {
        window.get_scroll_wheel().map_or((0.0, 0.0), |(x, y)| {
            (f64::from(x) / WHEEL_NOTCH, f64::from(y) / WHEEL_NOTCH)
        })
    }

    /// `1` = left, `2` = right, `3` = middle; anything else is not a button.
    fn slot(button: i64) -> Option<usize> {
        match button {
            1 => Some(0),
            2 => Some(1),
            3 => Some(2),
            _ => None,
        }
    }

    pub fn is_down(&self, button: i64) -> bool {
        Self::slot(button).is_some_and(|i| self.down[i])
    }

    pub fn was_pressed(&self, button: i64) -> bool {
        Self::slot(button).is_some_and(|i| self.pressed[i])
    }

    pub fn was_released(&self, button: i64) -> bool {
        Self::slot(button).is_some_and(|i| self.released[i])
    }

    /// Wheel movement since the last `pollEvents`, as `(x, y)`. Positive `y` is
    /// a scroll up / away from the user.
    pub fn wheel(&self) -> (f64, f64) {
        self.wheel
    }
}

/// Live engine state, created by `Window.create`.
pub struct Engine {
    window: Window,
    /// The window's framebuffer. minifb reads it as `0RGB` and ignores alpha.
    screen: Surface,
    /// Canvas registry; a slot is `None` only while its surface is on loan to
    /// a draw call (see [`Engine::draw_canvas`]). Handle `n` is index `n - 1`.
    canvases: Vec<Option<Surface>>,
    /// Bound render target: `None` is the screen, `Some(i)` a canvas index.
    target: Option<usize>,
    /// Font registry. Slot 0 is the system default, loaded on first use.
    fonts: Vec<Option<FontRes>>,
    background: [f64; 4],
    st: GState,
    stack: Vec<Saved>,
    /// Bilinear rather than nearest sampling for transformed blits.
    linear_filter: bool,
    /// Per-frame keyboard edges, latched by [`Engine::poll_events`].
    keys: KeyState,
    /// Per-frame mouse edges and wheel, latched by [`Engine::poll_events`].
    mouse: MouseState,
    /// Instant the next presented frame should be released at — the single
    /// 60 FPS pacing point (see [`Engine::present`]).
    next_frame: Instant,
    /// OS scale factor, refreshed each frame so dragging a window between
    /// monitors of different DPI is picked up.
    scale: f64,
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
    // pumps the queue; `Keyboard.getTextInput` and `KeyState::sync` drain what
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

impl Engine {
    /// `true` while the OS window is open and Escape isn't held.
    pub fn is_open(&self) -> bool {
        self.window.is_open() && !self.window.is_key_down(Key::Escape)
    }

    /// Cursor position in window pixels, clamped to the window bounds.
    pub fn mouse_pos(&self) -> (f64, f64) {
        self.window
            .get_mouse_pos(MouseMode::Clamp)
            .map(|(x, y)| (x as f64, y as f64))
            .unwrap_or((0.0, 0.0))
    }

    /// This frame's mouse state: held buttons, edges, and wheel movement.
    ///
    /// Everything comes from the same `pollEvents` snapshot, so `isDown`,
    /// `wasPressed` and `wasReleased` always describe one consistent instant.
    pub fn mouse(&self) -> &MouseState {
        &self.mouse
    }

    /// Swap the cursor image. Unknown names are rejected rather than silently
    /// ignored, so a typo surfaces at the call site.
    pub fn set_cursor(&mut self, style: &str) -> Result<(), String> {
        let cursor = match style {
            "arrow" => CursorStyle::Arrow,
            "ibeam" | "text" => CursorStyle::Ibeam,
            "crosshair" => CursorStyle::Crosshair,
            "hand" | "openhand" => CursorStyle::OpenHand,
            "grab" | "closedhand" => CursorStyle::ClosedHand,
            "resizeleftright" | "ew" => CursorStyle::ResizeLeftRight,
            "resizeupdown" | "ns" => CursorStyle::ResizeUpDown,
            "resizeall" | "move" => CursorStyle::ResizeAll,
            other => {
                return Err(format!(
                    "Mouse.setCursor: unknown cursor {other:?} — expected one of \
                     arrow, ibeam, crosshair, hand, grab, resizeleftright, \
                     resizeupdown, resizeall"
                ));
            }
        };
        self.window.set_cursor_style(cursor);
        Ok(())
    }

    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.window.set_cursor_visibility(visible);
    }

    pub fn is_key_down(&self, key: Key) -> bool {
        self.window.is_key_down(key)
    }

    /// The canonical names of every key held right now, in the keyboard
    /// module's table order. Unnameable keys are skipped.
    pub fn keys_held(&self) -> Vec<&'static str> {
        self.window
            .get_keys()
            .into_iter()
            .filter_map(keyboard::key_name)
            .collect()
    }

    /// This frame's keyboard edges (`wasPressed` / `wasReleased`).
    pub fn keys(&self) -> &KeyState {
        &self.keys
    }

    pub fn keys_mut(&mut self) -> &mut KeyState {
        &mut self.keys
    }

    /// The window's framebuffer dimensions as `(width, height)`.
    pub fn size(&self) -> (usize, usize) {
        (self.screen.w, self.screen.h)
    }

    /// The DPI scale factor, refreshed every `pollEvents`.
    pub fn dpi_scale(&self) -> f64 {
        self.scale
    }

    /// Pump the OS event queue without presenting a frame. Keeps
    /// `is_open` / input fresh at the top of the loop, and is the frame
    /// boundary the keyboard's press/release edges are measured against.
    pub fn poll_events(&mut self) {
        self.window.update();
        self.keys.sync(&self.window);
        self.mouse.sync(&self.window);
        self.sync_surface();
    }

    /// Match the framebuffer to the window, and refresh the scale factor.
    ///
    /// A resized window needs a new buffer before anything is drawn into it —
    /// `update_with_buffer` rejects a mismatch, and drawing into the old one
    /// would clip to the previous size. The scale is re-read here too, so
    /// dragging a window onto a monitor with different DPI is picked up.
    fn sync_surface(&mut self) {
        let (w, h) = self.window.get_size();

        if w > 0 && h > 0 && (w != self.screen.w || h != self.screen.h) {
            self.screen = Surface::opaque(w, h);
            // A scissor from the old size could be entirely outside the new
            // one, which would silently blank the frame.
            self.st.scissor = None;
        }

        self.scale = query_scale(&self.window);
    }

    pub fn set_title(&mut self, title: &str) {
        self.window.set_title(title);
    }

    pub fn position(&self) -> (i64, i64) {
        let (x, y) = self.window.get_position();

        (x as i64, y as i64)
    }

    pub fn set_position(&mut self, x: i64, y: i64) {
        self.window.set_position(x as isize, y as isize);
    }

    pub fn set_topmost(&mut self, topmost: bool) {
        self.window.topmost(topmost);
    }

    /// Whether the window has keyboard focus. A background app should usually
    /// stop animating.
    pub fn is_focused(&mut self) -> bool {
        self.window.is_active()
    }

    /// Present the framebuffer to the window, then sleep until the next 60 FPS
    /// deadline. This is the loop's single pacing point.
    pub fn present(&mut self) -> Result<(), String> {
        // Split the borrow so `update_with_buffer` can take `&mut window`
        // and `&buffer` at once.
        let Engine { window, screen, .. } = self;
        window
            .update_with_buffer(&screen.buf, screen.w, screen.h)
            .map_err(|e| format!("Graphics.present: {e}"))?;

        // Presenting pumps the OS queue a second time, and minifb clears its
        // scroll delta at the start of every pump. Sampling here is what stops
        // a wheel notch that arrived mid-frame from being wiped before
        // `pollEvents` gets to read it. Keys need no equivalent: their edges
        // are latched in the backend's callback as the messages arrive.
        self.mouse.observe(&self.window);

        // Pace to 60 FPS: sleep off whatever time is left in this frame.
        let now = Instant::now();
        if now < self.next_frame {
            std::thread::sleep(self.next_frame - now);
        }
        // Schedule the next deadline; if we fell behind (a slow frame), resync
        // from now so we don't try to "catch up" with a burst of zero-sleep
        // frames.
        self.next_frame += FRAME_DUR;
        let now = Instant::now();
        if self.next_frame < now {
            self.next_frame = now + FRAME_DUR;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Render targets
// ---------------------------------------------------------------------------

impl Engine {
    fn target_size(&self) -> (usize, usize) {
        match self.target {
            None => (self.screen.w, self.screen.h),
            Some(i) => self.canvases[i]
                .as_ref()
                .map(|s| (s.w, s.h))
                .unwrap_or((0, 0)),
        }
    }

    fn target_mut(&mut self) -> &mut Surface {
        match self.target {
            None => &mut self.screen,
            Some(i) => self.canvases[i]
                .as_mut()
                .expect("the bound canvas is never on loan during a draw"),
        }
    }

    /// The paint for the current state, with the scissor already reduced to the
    /// target's bounds.
    fn paint(&self) -> Paint {
        let (w, h) = self.target_size();
        let bounds = Rect::surface(w, h);
        Paint {
            color: narrow(self.st.color),
            blend: self.st.blend,
            clip: match self.st.scissor {
                Some(s) => s.intersect(&bounds),
                None => bounds,
            },
            antialias: self.st.smooth,
            linear_filter: self.linear_filter,
        }
    }

    /// Allocate a canvas and return its handle (`1`-based).
    pub fn new_canvas(&mut self, w: i64, h: i64) -> Result<i64, String> {
        if w <= 0 || h <= 0 {
            return Err("Graphics.newCanvas: width and height must be positive".into());
        }
        // Guard against a typo turning into a multi-gigabyte allocation.
        if w > 16384 || h > 16384 {
            return Err("Graphics.newCanvas: dimensions may not exceed 16384".into());
        }
        self.canvases
            .push(Some(Surface::new(w as usize, h as usize)));
        Ok(self.canvases.len() as i64)
    }

    fn canvas_index(&self, handle: i64, func: &str) -> Result<usize, String> {
        if handle < 1 || handle as usize > self.canvases.len() {
            return Err(format!("{func}: no canvas with handle {handle}"));
        }
        Ok(handle as usize - 1)
    }

    /// Bind a canvas as the render target. `None` (or handle `0`) restores the
    /// screen.
    pub fn set_canvas(&mut self, handle: Option<i64>) -> Result<(), String> {
        self.target = match handle {
            None | Some(0) => None,
            Some(h) => Some(self.canvas_index(h, "Graphics.setCanvas")?),
        };
        Ok(())
    }

    /// The bound canvas handle, or `0` for the screen.
    pub fn get_canvas(&self) -> i64 {
        self.target.map(|i| i as i64 + 1).unwrap_or(0)
    }

    /// Composite a canvas onto the current target.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_canvas(
        &mut self,
        handle: i64,
        x: f64,
        y: f64,
        angle: f64,
        sx: f64,
        sy: f64,
        ox: f64,
        oy: f64,
    ) -> Result<(), String> {
        let idx = self.canvas_index(handle, "Graphics.draw")?;
        if self.target == Some(idx) {
            return Err("Graphics.draw: a canvas cannot be drawn onto itself".into());
        }

        let local = Transform::translation(x, y)
            .then(&Transform::rotation(angle))
            .then(&Transform::scaling(sx, sy))
            .then(&Transform::translation(-ox, -oy));
        let xform = self.st.transform.then(&local);
        let paint = self.paint();

        // Lift the source out of the registry so the source and destination
        // borrows are provably disjoint, then put it straight back.
        let src = self.canvases[idx]
            .take()
            .expect("a canvas is only on loan during its own draw");
        raster::blit_surface(self.target_mut(), &src, &xform, &paint);
        self.canvases[idx] = Some(src);
        Ok(())
    }

    /// Decode a PNG into the surface registry and return its handle.
    ///
    /// Images and canvases share one registry, so the handle works with
    /// `draw`, `drawFrame`, `imageSize`, and even `setCanvas` — a loaded image
    /// is simply a canvas that started life with pixels in it.
    pub fn new_image(&mut self, path: &str) -> Result<i64, String> {
        let surface = crate::image::load(path)?;
        self.canvases.push(Some(surface));
        Ok(self.canvases.len() as i64)
    }

    /// Pixel dimensions of an image or canvas.
    pub fn image_size(&self, handle: i64) -> Result<(i64, i64), String> {
        let idx = self.canvas_index(handle, "Graphics.imageSize")?;
        let surface = self.canvases[idx]
            .as_ref()
            .ok_or("Graphics.imageSize: the image is on loan to a draw call")?;
        Ok((surface.w as i64, surface.h as i64))
    }

    /// Composite one cell of an image onto the current target — the
    /// spritesheet draw. `frame` selects the source rectangle; the rest
    /// positions it exactly like [`Engine::draw_canvas`].
    #[allow(clippy::too_many_arguments)]
    pub fn draw_frame(
        &mut self,
        handle: i64,
        frame: Rect,
        x: f64,
        y: f64,
        angle: f64,
        sx: f64,
        sy: f64,
        ox: f64,
        oy: f64,
    ) -> Result<(), String> {
        let idx = self.canvas_index(handle, "Graphics.drawFrame")?;
        if self.target == Some(idx) {
            return Err("Graphics.drawFrame: an image cannot be drawn onto itself".into());
        }

        let local = Transform::translation(x, y)
            .then(&Transform::rotation(angle))
            .then(&Transform::scaling(sx, sy))
            .then(&Transform::translation(-ox, -oy));
        let xform = self.st.transform.then(&local);
        let paint = self.paint();

        let src = self.canvases[idx]
            .take()
            .expect("an image is only on loan during its own draw");
        raster::blit_surface_sub(self.target_mut(), &src, frame, &xform, &paint);
        self.canvases[idx] = Some(src);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Graphics state
// ---------------------------------------------------------------------------

impl Engine {
    pub fn set_color(&mut self, r: f64, g: f64, b: f64, a: f64) {
        self.st.color = [r, g, b, a];
    }

    pub fn color(&self) -> (f64, f64, f64, f64) {
        let c = self.st.color;
        (c[0], c[1], c[2], c[3])
    }

    pub fn set_background_color(&mut self, r: f64, g: f64, b: f64, a: f64) {
        self.background = [r, g, b, a];
    }

    pub fn background_color(&self) -> (f64, f64, f64, f64) {
        let c = self.background;
        (c[0], c[1], c[2], c[3])
    }

    pub fn set_line_width(&mut self, w: f64) {
        self.st.line_width = w.max(0.0);
    }

    pub fn line_width(&self) -> f64 {
        self.st.line_width
    }

    pub fn set_line_style(&mut self, style: &str) -> Result<(), String> {
        self.st.smooth = match style {
            "smooth" => true,
            "rough" => false,
            other => {
                return Err(format!(
                    "Graphics.setLineStyle: unknown style `{other}` \
                     (expected \"smooth\" or \"rough\")"
                ));
            }
        };
        Ok(())
    }

    pub fn line_style(&self) -> &'static str {
        if self.st.smooth { "smooth" } else { "rough" }
    }

    pub fn set_line_join(&mut self, join: &str) -> Result<(), String> {
        self.st.line_join =
            LineJoin::parse(join).map_err(|e| format!("Graphics.setLineJoin: {e}"))?;
        Ok(())
    }

    pub fn line_join(&self) -> &'static str {
        self.st.line_join.name()
    }

    pub fn set_blend_mode(&mut self, mode: &str) -> Result<(), String> {
        self.st.blend =
            BlendMode::parse(mode).map_err(|e| format!("Graphics.setBlendMode: {e}"))?;
        Ok(())
    }

    pub fn blend_mode(&self) -> &'static str {
        self.st.blend.name()
    }

    pub fn set_default_filter(&mut self, mode: &str) -> Result<(), String> {
        self.linear_filter = match mode {
            "linear" => true,
            "nearest" => false,
            other => {
                return Err(format!(
                    "Graphics.setDefaultFilter: unknown filter `{other}` \
                     (expected \"linear\" or \"nearest\")"
                ));
            }
        };
        Ok(())
    }

    pub fn default_filter(&self) -> &'static str {
        if self.linear_filter {
            "linear"
        } else {
            "nearest"
        }
    }

    /// The device-space bounding box of a local rectangle under the current
    /// transform. Scissors follow the transform, so a clipped scroll view keeps
    /// working when its parent is translated.
    fn device_rect(&self, x: f64, y: f64, w: f64, h: f64) -> Rect {
        let t = &self.st.transform;
        let corners = [
            t.apply(x, y),
            t.apply(x + w, y),
            t.apply(x + w, y + h),
            t.apply(x, y + h),
        ];
        let mut r = Rect {
            x0: f64::INFINITY,
            y0: f64::INFINITY,
            x1: f64::NEG_INFINITY,
            y1: f64::NEG_INFINITY,
        };
        for (px, py) in corners {
            r.x0 = r.x0.min(px);
            r.x1 = r.x1.max(px);
            r.y0 = r.y0.min(py);
            r.y1 = r.y1.max(py);
        }
        r
    }

    /// `None` disables clipping entirely.
    pub fn set_scissor(&mut self, rect: Option<(f64, f64, f64, f64)>) {
        let device = rect.map(|(x, y, w, h)| self.device_rect(x, y, w, h));
        self.st.scissor = device;
    }

    /// Narrow the clip to the intersection with an existing one. This is what
    /// makes nested clipping — a scroll view inside a scroll view — compose.
    pub fn intersect_scissor(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let device = self.device_rect(x, y, w, h);
        self.st.scissor = Some(match self.st.scissor {
            Some(existing) => existing.intersect(&device),
            None => device,
        });
    }

    /// The active clip in device coordinates, defaulting to the whole target.
    pub fn scissor(&self) -> (f64, f64, f64, f64) {
        let (w, h) = self.target_size();
        let r = self.st.scissor.unwrap_or(Rect::surface(w, h));
        (r.x0, r.y0, r.x1 - r.x0, r.y1 - r.y0)
    }

    /// Restore every default: colour, line settings, blend mode, filter,
    /// scissor, transform, and font selection. Canvases and loaded fonts
    /// survive, since they are resources rather than state.
    pub fn reset(&mut self) {
        self.st = GState::default();
        self.stack.clear();
        self.background = [0.0, 0.0, 0.0, 1.0];
        self.linear_filter = true;
        self.target = None;
    }
}

// ---------------------------------------------------------------------------
// Coordinate system
// ---------------------------------------------------------------------------

impl Engine {
    pub fn push(&mut self, all: bool) {
        self.stack.push(if all {
            Saved::All(Box::new(self.st.clone()))
        } else {
            Saved::TransformOnly(self.st.transform)
        });
    }

    pub fn pop(&mut self) -> Result<(), String> {
        match self.stack.pop() {
            Some(Saved::TransformOnly(t)) => self.st.transform = t,
            Some(Saved::All(s)) => self.st = *s,
            None => return Err("Graphics.pop: the transform stack is empty".into()),
        }
        Ok(())
    }

    pub fn stack_depth(&self) -> i64 {
        self.stack.len() as i64
    }

    pub fn origin(&mut self) {
        self.st.transform = Transform::IDENTITY;
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        self.st.transform = self.st.transform.then(&Transform::translation(dx, dy));
    }

    pub fn scale(&mut self, sx: f64, sy: f64) {
        self.st.transform = self.st.transform.then(&Transform::scaling(sx, sy));
    }

    pub fn rotate(&mut self, angle: f64) {
        self.st.transform = self.st.transform.then(&Transform::rotation(angle));
    }

    pub fn shear(&mut self, kx: f64, ky: f64) {
        self.st.transform = self.st.transform.then(&Transform::shearing(kx, ky));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_transform(&mut self, a: f64, b: f64, c: f64, d: f64, tx: f64, ty: f64) {
        let t = Transform { a, b, c, d, tx, ty };
        self.st.transform = self.st.transform.then(&t);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn replace_transform(&mut self, a: f64, b: f64, c: f64, d: f64, tx: f64, ty: f64) {
        self.st.transform = Transform { a, b, c, d, tx, ty };
    }

    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64) {
        self.st.transform.apply(x, y)
    }

    pub fn inverse_transform_point(&self, x: f64, y: f64) -> Result<(f64, f64), String> {
        self.st
            .transform
            .inverse()
            .map(|inv| inv.apply(x, y))
            .ok_or_else(|| {
                "Graphics.inverseTransformPoint: the current transform is not invertible \
                 (a scale factor is zero)"
                    .to_string()
            })
    }
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

impl Engine {
    pub fn clear(&mut self, color: Option<(f64, f64, f64, f64)>) {
        let c = narrow(match color {
            Some((r, g, b, a)) => [r, g, b, a],
            None => self.background,
        });
        let (w, h) = self.target_size();
        let clip = match self.st.scissor {
            Some(s) => s.intersect(&Rect::surface(w, h)),
            None => Rect::surface(w, h),
        };
        self.target_mut().clear(c, clip);
    }

    /// The shared tail of every shape call: transform the path into device
    /// space, then either fill it or expand it into a stroke.
    fn draw_path(&mut self, mode: &str, local: &[Point], closed: bool) -> Result<(), String> {
        let t = self.st.transform;
        let device: Vec<Point> = local.iter().map(|&(x, y)| t.apply(x, y)).collect();

        let paths = match mode {
            "fill" => vec![device],
            "line" => {
                // Line width is a local-space quantity in Love2D, so it scales
                // with the transform just like the geometry does.
                let w = self.st.line_width * t.mean_scale();
                geom::stroke(&device, closed, w, self.st.line_join)
            }
            other => {
                return Err(format!(
                    "draw mode must be \"fill\" or \"line\", got `{other}`"
                ));
            }
        };

        let paint = self.paint();
        raster::fill_paths(self.target_mut(), &paths, &paint);
        Ok(())
    }

    /// Segment budget for a curve of local radius `r` under the current
    /// transform.
    fn segments_for(&self, r: f64) -> usize {
        geom::curve_segments(r * self.st.transform.mean_scale())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rectangle(
        &mut self,
        mode: &str,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        rx: f64,
        ry: f64,
    ) -> Result<(), String> {
        let path = if rx > 0.0 || ry > 0.0 {
            let segs = self.segments_for(rx.abs().max(ry.abs()));
            geom::rounded_rect_path(x, y, w, h, rx, ry, segs)
        } else {
            geom::rect_path(x, y, w, h)
        };
        self.draw_path(mode, &path, true)
    }

    pub fn circle(
        &mut self,
        mode: &str,
        x: f64,
        y: f64,
        radius: f64,
        segments: Option<i64>,
    ) -> Result<(), String> {
        self.ellipse(mode, x, y, radius, radius, segments)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(
        &mut self,
        mode: &str,
        x: f64,
        y: f64,
        rx: f64,
        ry: f64,
        segments: Option<i64>,
    ) -> Result<(), String> {
        let segs = match segments {
            Some(n) if n >= 3 => n as usize,
            Some(n) => return Err(format!("segment count must be at least 3, got {n}")),
            None => self.segments_for(rx.abs().max(ry.abs())),
        };
        let path = geom::ellipse_path(x, y, rx, ry, segs);
        self.draw_path(mode, &path, true)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn arc(
        &mut self,
        mode: &str,
        x: f64,
        y: f64,
        radius: f64,
        angle1: f64,
        angle2: f64,
        arctype: &str,
    ) -> Result<(), String> {
        let kind = ArcType::parse(arctype)?;
        let segs = self.segments_for(radius);
        let (path, closed) = geom::arc_path(x, y, radius, angle1, angle2, segs, kind);
        self.draw_path(mode, &path, closed)
    }

    pub fn polygon(&mut self, mode: &str, points: &[Point]) -> Result<(), String> {
        if points.len() < 3 {
            return Err("need at least 3 points".into());
        }
        self.draw_path(mode, points, true)
    }

    pub fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) -> Result<(), String> {
        self.draw_path("line", &[(x1, y1), (x2, y2)], false)
    }

    pub fn polyline(&mut self, points: &[Point]) -> Result<(), String> {
        if points.len() < 2 {
            return Err("need at least 2 points".into());
        }
        self.draw_path("line", points, false)
    }

    /// Draw one-pixel points. Each lands on the device pixel containing the
    /// transformed position, so points stay crisp under any transform.
    pub fn points(&mut self, points: &[Point]) {
        let t = self.st.transform;
        let paths: Vec<Vec<Point>> = points
            .iter()
            .map(|&(x, y)| {
                let (dx, dy) = t.apply(x, y);
                let (px, py) = (dx.floor(), dy.floor());
                vec![
                    (px, py),
                    (px + 1.0, py),
                    (px + 1.0, py + 1.0),
                    (px, py + 1.0),
                ]
            })
            .collect();
        let paint = self.paint();
        raster::fill_paths(self.target_mut(), &paths, &paint);
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

impl Engine {
    /// Load a typeface. `path` of `None` uses the system default face.
    /// Returns the new font's handle.
    pub fn new_font(&mut self, size: f64, path: Option<&str>) -> Result<i64, String> {
        let res = match path {
            Some(p) => FontRes::from_file(p, size)?,
            None => font::load_default(size).ok_or_else(no_system_font)?,
        };
        self.fonts.push(Some(res));
        Ok(self.fonts.len() as i64 - 1)
    }

    pub fn set_font(&mut self, handle: i64) -> Result<(), String> {
        if handle < 0 || handle as usize >= self.fonts.len() {
            return Err(format!("Graphics.setFont: no font with handle {handle}"));
        }
        self.st.font = handle as usize;
        Ok(())
    }

    pub fn get_font(&self) -> i64 {
        self.st.font as i64
    }

    /// Make sure the selected font slot is populated, loading the system
    /// default on first use.
    fn ensure_font(&mut self) -> Result<(), String> {
        let i = self.st.font;
        if self.fonts.get(i).is_some_and(|f| f.is_some()) {
            return Ok(());
        }
        if i != 0 {
            return Err(format!("no font with handle {i}"));
        }
        self.fonts[0] = Some(font::load_default(font::DEFAULT_SIZE).ok_or_else(no_system_font)?);
        Ok(())
    }

    /// Borrow the render target and the active font at once. They live in
    /// disjoint fields, so destructuring is what makes the two `&mut`s legal.
    fn target_and_font(&mut self) -> (&mut Surface, &mut FontRes) {
        let Engine {
            screen,
            canvases,
            target,
            fonts,
            st,
            ..
        } = self;
        let surf = match target {
            None => screen,
            Some(i) => canvases[*i]
                .as_mut()
                .expect("the bound canvas is never on loan during a draw"),
        };
        let font = fonts[st.font]
            .as_mut()
            .expect("ensure_font ran before this call");
        (surf, font)
    }

    pub fn font_height(&mut self) -> Result<f64, String> {
        self.ensure_font()?;
        Ok(self.fonts[self.st.font].as_ref().unwrap().height())
    }

    pub fn text_width(&mut self, text: &str) -> Result<f64, String> {
        self.ensure_font()?;
        let i = self.st.font;
        Ok(self.fonts[i].as_mut().unwrap().measure(text))
    }

    /// Draw a single line of text with its top-left corner at `(x, y)`.
    pub fn print(&mut self, text: &str, x: f64, y: f64) -> Result<(), String> {
        self.ensure_font()?;
        let paint = self.paint();
        let xform = self.st.transform;
        let (surf, font) = self.target_and_font();

        let mut cursor_y = y;
        for line in text.split('\n') {
            draw_line(surf, font, line, x, cursor_y, &xform, &paint);
            cursor_y += font.height();
        }
        Ok(())
    }

    /// Draw word-wrapped, aligned text inside a `limit`-wide box anchored at
    /// `(x, y)`.
    pub fn printf(
        &mut self,
        text: &str,
        x: f64,
        y: f64,
        limit: f64,
        align: &str,
    ) -> Result<(), String> {
        let align = Align::parse(align)?;
        self.ensure_font()?;
        let paint = self.paint();
        let xform = self.st.transform;
        let (surf, font) = self.target_and_font();

        let lines = font.wrap(text, limit);
        let mut cursor_y = y;
        for line in &lines {
            let width = font.layout(line).1;
            draw_line(
                surf,
                font,
                line,
                x + align.offset(width, limit),
                cursor_y,
                &xform,
                &paint,
            );
            cursor_y += font.height();
        }
        Ok(())
    }
}

/// Narrow a state colour to the rasterizer's working precision.
fn narrow(c: [f64; 4]) -> [f32; 4] {
    [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32]
}

fn no_system_font() -> String {
    "no font available — the engine found no system typeface to fall back on; \
     load one explicitly with Graphics.newFont(size, path)"
        .to_string()
}

/// Blit one laid-out line. `y` is the line's *top*, matching how Love2D's
/// `print` anchors text.
fn draw_line(
    surf: &mut Surface,
    font: &mut FontRes,
    text: &str,
    x: f64,
    y: f64,
    xform: &Transform,
    paint: &Paint,
) {
    if text.is_empty() {
        return;
    }
    let baseline = y + font.ascent();
    let (glyphs, _) = font.layout(text);
    for (ch, pen) in glyphs {
        let Some(glyph) = font.glyph(ch) else {
            continue;
        };
        if glyph.mask.w == 0 || glyph.mask.h == 0 {
            continue; // whitespace carries advance but no pixels
        }
        let placement = Transform::translation(x + pen + glyph.left, baseline + glyph.top);
        raster::blit_mask(surf, &glyph.mask, &xform.then(&placement), paint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_matches_love_defaults() {
        let s = GState::default();
        assert_eq!(s.color, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(s.line_width, 1.0);
        assert_eq!(s.line_join, LineJoin::Miter);
        assert_eq!(s.blend, BlendMode::Alpha);
        assert!(s.smooth);
        assert!(s.scissor.is_none());
        assert_eq!(s.transform, Transform::IDENTITY);
        assert_eq!(s.font, 0);
    }
}
