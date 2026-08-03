//! The OS window and the frame loop: opening, sizing, presenting,
//! and the per-platform DPI scale queries that back them.

mod canvas;
mod input;

use crate::raster::Surface;
use minifb::{Key, Window};
use std::time::{Duration, Instant};

use super::*;

/// Ask the OS for this window's scale factor, with 1.0 meaning "96 DPI".
///
/// minifb has no DPI query, so this goes through the native window handle.
/// Only Windows is wired up; the others report 1.0 rather than guessing, which
/// at least makes the limitation visible instead of subtly wrong.
#[cfg(windows)]
pub(crate) fn query_scale(window: &Window) -> f64 {
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
pub(crate) fn query_scale(_window: &Window) -> f64 {
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
pub(crate) fn declare_dpi_aware() {
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
pub(crate) fn declare_dpi_aware() {}

/// Target frame time for the 60 FPS cap.
pub(crate) const FRAME_DUR: Duration = Duration::from_nanos(1_000_000_000 / 60);

impl Engine {
    /// `true` while the OS window is open and Escape isn't held.
    pub fn is_open(&self) -> bool {
        self.window.is_open() && !self.window.is_key_down(Key::Escape)
    }

    /// The window's framebuffer dimensions as `(width, height)`.
    pub fn size(&self) -> (usize, usize) {
        (self.screen.w, self.screen.h)
    }

    /// The DPI scale factor, refreshed every `pollEvents`.
    pub fn dpi_scale(&self) -> f64 {
        self.scale
    }

    /// Match the framebuffer to the window, and refresh the scale factor.
    ///
    /// A resized window needs a new buffer before anything is drawn into it —
    /// `update_with_buffer` rejects a mismatch, and drawing into the old one
    /// would clip to the previous size. The scale is re-read here too, so
    /// dragging a window onto a monitor with different DPI is picked up.
    /// Returns the new size when the framebuffer was actually replaced, which
    /// is what a `Resized` event reports.
    pub(crate) fn sync_surface(&mut self) -> Option<(usize, usize)> {
        let (w, h) = self.window.get_size();
        let mut resized = None;

        if w > 0 && h > 0 && (w != self.screen.w || h != self.screen.h) {
            self.screen = Surface::opaque(w, h);
            // A scissor from the old size could be entirely outside the new
            // one, which would silently blank the frame.
            self.st.scissor = None;
            resized = Some((w, h));
        }

        self.scale = query_scale(&self.window);

        resized
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

    // ---------------------------------------------------------------------------
    // Render targets
    // ---------------------------------------------------------------------------
}

// ---------------------------------------------------------------------------
// Graphics state
// ---------------------------------------------------------------------------
