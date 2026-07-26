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

use minifb::{Key, MouseButton, MouseMode, Scale, Window, WindowOptions};

use crate::font::{self, Align, FontRes};
use crate::geom::{self, ArcType, LineJoin, Point, Transform};
use crate::keyboard::{self, KeyState, TextCollector};
use crate::raster::{self, BlendMode, Paint, Rect, Surface};

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
    /// Instant the next presented frame should be released at — the single
    /// 60 FPS pacing point (see [`Engine::present`]).
    next_frame: Instant,
}

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
}

/// Create the window and framebuffer. Replaces any existing window.
pub fn create(width: i64, height: i64, title: &str) -> Result<(), String> {
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
            resize: false,
            scale: Scale::X1,
            ..WindowOptions::default()
        },
    )
    .map_err(|e| format!("Window.create: {e}"))?;

    // We pace the loop ourselves in `present` (a single sleep per frame), so
    // leave minifb's own limiter off — otherwise both `update` and
    // `update_with_buffer` would each block and halve the frame rate.
    window.set_target_fps(0);

    // Typed text arrives through this callback while `update` pumps the queue;
    // `Keyboard.getTextInput` drains what it collects.
    window.set_input_callback(Box::new(TextCollector));
    keyboard::reset_text();

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
            next_frame: Instant::now() + FRAME_DUR,
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

    /// Whether `button` is currently held. `1` = left, `2` = right,
    /// `3` = middle. Returns `false` for unrecognised button indices.
    pub fn mouse_is_down(&self, button: i64) -> bool {
        let btn = match button {
            1 => MouseButton::Left,
            2 => MouseButton::Right,
            3 => MouseButton::Middle,
            _ => return false,
        };
        self.window.get_mouse_down(btn)
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

    /// The DPI scale factor. minifb has no portable way to report this, so the
    /// engine always works in physical pixels and reports a scale of 1.
    pub fn dpi_scale(&self) -> f64 {
        1.0
    }

    /// Pump the OS event queue without presenting a frame. Keeps
    /// `is_open` / input fresh at the top of the loop, and is the frame
    /// boundary the keyboard's press/release edges are measured against.
    pub fn poll_events(&mut self) {
        self.window.update();
        self.keys.sync(&self.window);
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
        self.st.line_join = LineJoin::parse(join).map_err(|e| format!("Graphics.setLineJoin: {e}"))?;
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
        if self.linear_filter { "linear" } else { "nearest" }
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
        let Some(glyph) = font.glyph(ch) else { continue };
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
