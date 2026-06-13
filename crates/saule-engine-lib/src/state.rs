//! Shared engine state: the OS window, its pixel framebuffer, and the
//! software rasterizers that draw into it.
//!
//! The interpreter is single-threaded and calls every native symbol from the
//! same thread, so the state lives in a `thread_local!`. That sidesteps the
//! `Send`/`Sync` requirements a `static` would impose (minifb's `Window` is
//! not `Send`), and matches the actual call pattern exactly.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use minifb::{Key, MouseButton, MouseMode, Scale, Window, WindowOptions};

/// Target frame time for the 60 FPS cap.
const FRAME_DUR: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Live engine state, created by `Window.create`.
pub struct Engine {
    window: Window,
    /// `0RGB` pixels, `width * height` long. minifb ignores the top byte.
    buffer: Vec<u32>,
    width: usize,
    height: usize,
    /// Current draw colour for shapes, packed `0RGB`.
    color: u32,
    line_width: f32,
    /// Instant the next presented frame should be released at — the single
    /// 60 FPS pacing point (see [`Engine::present`]).
    next_frame: Instant,
}

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
}

/// Pack three `0.0..=1.0` channels into a `0RGB` pixel.
pub fn pack_color(r: f64, g: f64, b: f64) -> u32 {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (c(r) << 16) | (c(g) << 8) | c(b)
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

    ENGINE.with(|cell| {
        *cell.borrow_mut() = Some(Engine {
            window,
            buffer: vec![0; width * height],
            width,
            height,
            color: 0x00FF_FFFF, // default draw colour: white
            line_width: 1.0,
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

    /// The window's framebuffer dimensions as `(width, height)`.
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Pump the OS event queue without presenting a frame. Keeps
    /// `is_open` / input fresh at the top of the loop.
    pub fn poll_events(&mut self) {
        self.window.update();
    }

    /// Set the current draw colour for subsequent shapes.
    pub fn set_color(&mut self, r: f64, g: f64, b: f64) {
        self.color = pack_color(r, g, b);
    }

    /// Set the current draw colour for subsequent shapes.
    pub fn set_line_width(&mut self, w: f64) {
        self.line_width = w.max(0.0) as f32;
    }

    /// Fill the whole framebuffer with `(r, g, b)`.
    pub fn clear(&mut self, r: f64, g: f64, b: f64) {
        let c = pack_color(r, g, b);
        self.buffer.iter_mut().for_each(|p| *p = c);
    }

    /// Present the framebuffer to the window, then sleep until the next 60 FPS
    /// deadline. This is the loop's single pacing point.
    pub fn present(&mut self) -> Result<(), String> {
        // Split the borrow so `update_with_buffer` can take `&mut window`
        // and `&buffer` at once.
        let Engine {
            window,
            buffer,
            width,
            height,
            ..
        } = self;
        window
            .update_with_buffer(buffer, *width, *height)
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

    /// Set one pixel in the current colour, ignoring out-of-bounds writes.
    fn put(&mut self, x: i32, y: i32) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            self.buffer[y as usize * self.width + x as usize] = self.color;
        }
    }

    pub fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        let lw = self.line_width as f64;
        if lw <= 1.0 {
            self.bresenham(
                x1.round() as i32,
                y1.round() as i32,
                x2.round() as i32,
                y2.round() as i32,
            );
        } else {
            let dx = x2 - x1;
            let dy = y2 - y1;
            let len = (dx * dx + dy * dy).sqrt().max(1.0);
            // Two stamps per pixel of length avoids visible gaps.
            let steps = (len * 2.0).round() as i32;
            for i in 0..=steps {
                let t = i as f64 / steps as f64;
                let cx = x1 + t * dx;
                let cy = y1 + t * dy;
                self.circle("fill", cx, cy, lw * 0.5);
            }
        }
    }

    /// Bresenham integer line rasterizer — always produces a 1-pixel stroke.
    fn bresenham(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32) {
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1i32 } else { -1 };
        let sy = if y0 < y1 { 1i32 } else { -1 };
        let mut err = dx - dy;
        loop {
            self.put(x0, y0);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                x0 += sx;
            }
            if e2 < dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Draw a rectangle. `fill` paints the interior; any other mode draws a
    /// 1px outline.
    pub fn rectangle(&mut self, mode: &str, x: f64, y: f64, w: f64, h: f64) {
        let x0 = x.round() as i32;
        let y0 = y.round() as i32;
        let x1 = (x + w).round() as i32;
        let y1 = (y + h).round() as i32;

        if mode == "fill" {
            for py in y0..y1 {
                for px in x0..x1 {
                    self.put(px, py);
                }
            }
        } else {
            for px in x0..x1 {
                self.put(px, y0);
                self.put(px, y1 - 1);
            }
            for py in y0..y1 {
                self.put(x0, py);
                self.put(x1 - 1, py);
            }
        }
    }

    /// Draw a circle. `fill` paints the disc; any other mode draws a ~1px
    /// ring.
    pub fn circle(&mut self, mode: &str, cx: f64, cy: f64, radius: f64) {
        if radius <= 0.0 {
            return;
        }
        let r = radius;
        let min_x = (cx - r).floor() as i32;
        let max_x = (cx + r).ceil() as i32;
        let min_y = (cy - r).floor() as i32;
        let max_y = (cy + r).ceil() as i32;
        let fill = mode == "fill";

        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let dx = px as f64 + 0.5 - cx;
                let dy = py as f64 + 0.5 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                let inside = if fill {
                    dist <= r
                } else {
                    (dist - r).abs() <= 1.0
                };
                if inside {
                    self.put(px, py);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pack_color;

    #[test]
    fn packs_primary_colors() {
        assert_eq!(pack_color(1.0, 0.0, 0.0), 0x00FF_0000);
        assert_eq!(pack_color(0.0, 1.0, 0.0), 0x0000_FF00);
        assert_eq!(pack_color(0.0, 0.0, 1.0), 0x0000_00FF);
        assert_eq!(pack_color(1.0, 1.0, 1.0), 0x00FF_FFFF);
    }

    #[test]
    fn clamps_out_of_range() {
        assert_eq!(pack_color(2.0, -1.0, 0.5), 0x00FF_0080);
    }
}
