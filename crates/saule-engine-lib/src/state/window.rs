//! The OS window and the frame loop: opening, sizing, presenting,
//! and the per-platform DPI scale queries that back them.

mod input;

use minifb::{Key, Window};
use std::time::{Duration, Instant};

use super::*;

/// Ask the OS for this window's scale factor, with 1.0 meaning "96 DPI".
///
/// minifb has no DPI query, so this goes through the native window handle.
/// Only Windows is wired up; the others report 1.0 rather than guessing, which
/// at least makes the limitation visible instead of subtly wrong.
///
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

/// Ask the NSWindow for its Retina backing scale.
///
/// minifb has no DPI query and its own macOS `scale_factor` is the integer
/// window magnification from [`minifb::Scale`], unrelated to this — so the
/// engine used to report `1.0` here and draw a point-resolution framebuffer
/// that the OS then magnified. On a Retina display that is a soft, visibly
/// pixelated UI, worst of all in text.
///
/// The backend can do better than that: it presents the framebuffer as a Metal
/// texture on an `MTKView`, whose drawable is already sized in *physical*
/// pixels. Handing it a framebuffer at physical resolution (see
/// [`Engine::backing_scale`]) makes that mapping 1:1 instead of a 2× upscale.
///
/// `mfb_open` returns an `OSXWindow`, which is an `NSWindow` subclass, so this
/// is a plain `backingScaleFactor` message. It goes through the Objective-C
/// runtime directly rather than pulling in an `objc` crate for one selector.
#[cfg(target_os = "macos")]
pub(crate) fn query_scale(window: &Window) -> f64 {
    use std::ffi::{c_char, c_void};

    unsafe extern "C" {
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn objc_msgSend();
    }

    let handle = window.get_window_handle();
    if handle.is_null() {
        return 1.0;
    }

    // Safety: `handle` is minifb's live `OSXWindow*` (an `NSWindow`), and
    // `backingScaleFactor` is a no-argument property getter returning CGFloat.
    // `objc_msgSend` has no single Rust type, so it is transmuted to the exact
    // signature of the call being made — which is what the ABI requires on
    // aarch64, where a mismatched cast would pass arguments in the wrong
    // registers.
    let scale = unsafe {
        let sel = sel_registerName(c"backingScaleFactor".as_ptr());
        if sel.is_null() {
            return 1.0;
        }
        let send: extern "C" fn(*mut c_void, *const c_void) -> f64 =
            std::mem::transmute(objc_msgSend as *const ());
        send(handle, sel)
    };

    // A window that is not on a screen yet can answer 0.
    if scale.is_finite() && scale >= 1.0 {
        scale
    } else {
        1.0
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
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

/// Framebuffer pixels per unit of the size and pointer coordinates minifb
/// reports, given the display's scale factor.
///
/// On macOS minifb works in *points*, so this is the Retina backing scale and
/// the framebuffer is allocated that much larger. On Windows the process is
/// per-monitor DPI aware and minifb already reports physical pixels, so
/// scaling again would double-count; X11 does no scaling here at all. Both are
/// therefore `1.0` regardless of what `getScale()` reports.
#[cfg(target_os = "macos")]
pub(crate) fn backing_scale_for(scale: f64) -> f64 {
    scale
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn backing_scale_for(_scale: f64) -> f64 {
    1.0
}

/// Frame budget a fresh window starts with — 60 FPS.
pub(crate) const DEFAULT_FRAME_DUR: Duration = Duration::from_nanos(1_000_000_000 / 60);

impl Engine {
    /// `true` while the OS window is open.
    ///
    /// Escape only ends the loop when `Window.setQuitOnEscape(true)` asked for
    /// it. Love2D quits on Escape by default; an application toolkit cannot
    /// live with that, because Escape is also how a modal, a menu, or an
    /// autocomplete popup is dismissed — and the app has no way to decline the
    /// engine's quit. The `Closed` event covers the real close.
    pub fn is_open(&self) -> bool {
        self.window.is_open() && !(self.quit_on_escape && self.window.is_key_down(Key::Escape))
    }

    pub fn set_quit_on_escape(&mut self, enable: bool) {
        self.quit_on_escape = enable;
    }

    pub fn quit_on_escape(&self) -> bool {
        self.quit_on_escape
    }

    /// The window's framebuffer dimensions as `(width, height)`.
    pub fn size(&self) -> (usize, usize) {
        (self.r.screen.w, self.r.screen.h)
    }

    /// The DPI scale factor, refreshed every `pollEvents`.
    pub fn dpi_scale(&self) -> f64 {
        self.scale
    }

    /// Cap the loop at `fps` frames per second; `0` removes the cap.
    ///
    /// The pacing sleep in [`Engine::present`] is the only thing throttling a
    /// Saule game loop, and it used to be a hardcoded 60 with no way to ask
    /// for less (a mostly idle UI on a laptop battery) or more (a 120 Hz
    /// display).
    pub fn set_target_fps(&mut self, fps: i64) -> Result<(), String> {
        if fps < 0 {
            return Err("Window.setTargetFPS: the rate cannot be negative".into());
        }
        if fps > 1000 {
            return Err("Window.setTargetFPS: the rate may not exceed 1000".into());
        }
        self.frame_dur = if fps == 0 {
            None
        } else {
            Some(Duration::from_secs_f64(1.0 / fps as f64))
        };
        self.next_frame = Instant::now() + self.frame_dur.unwrap_or_default();
        Ok(())
    }

    pub fn target_fps(&self) -> i64 {
        match self.frame_dur {
            None => 0,
            Some(d) => (1.0 / d.as_secs_f64()).round() as i64,
        }
    }

    /// How many framebuffer pixels there are per unit of the size and pointer
    /// coordinates minifb reports.
    ///
    /// On macOS minifb works in *points*, so this is the Retina backing scale
    /// and the framebuffer is allocated that much larger. On Windows the
    /// process is per-monitor DPI aware and minifb already reports physical
    /// pixels, so scaling again would double-count; X11 has no scaling here at
    /// all. Both of those are therefore `1.0` regardless of `getScale()`.
    pub(crate) fn backing_scale(&self) -> f64 {
        backing_scale_for(self.scale)
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
        // Read the scale first: the framebuffer size depends on it, so a window
        // dragged onto a denser monitor has to be re-sized in the same pass
        // that notices the density changed.
        self.scale = query_scale(&self.window);

        let (points_w, points_h) = self.window.get_size();
        let backing = self.backing_scale();
        let w = (points_w as f64 * backing).round() as usize;
        let h = (points_h as f64 * backing).round() as usize;

        let mut resized = None;

        if w > 0 && h > 0 && (w != self.r.screen.w || h != self.r.screen.h) {
            self.r.resize_screen(w, h);
            resized = Some((w, h));
        }

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

    /// Present the framebuffer to the window, then sleep until the next frame
    /// deadline. This is the loop's single pacing point.
    pub fn present(&mut self) -> Result<(), String> {
        // Split the borrow so `update_with_buffer` can take `&mut window`
        // and `&buffer` at once.
        let Engine { window, r, .. } = self;
        window
            .update_with_buffer(&r.screen.buf, r.screen.w, r.screen.h)
            .map_err(|e| format!("Graphics.present: {e}"))?;

        crate::timer::mark_frame();

        // Presenting pumps the OS queue a second time, and minifb clears its
        // scroll delta at the start of every pump. Sampling here is what stops
        // a wheel notch that arrived mid-frame from being wiped before
        // `pollEvents` gets to read it. Keys need no equivalent: their edges
        // are latched in the backend's callback as the messages arrive.
        self.mouse.observe(&self.window);

        let Some(budget) = self.frame_dur else {
            return Ok(()); // uncapped: present and go straight back round
        };

        // Pace the loop: sleep off whatever time is left in this frame.
        let now = Instant::now();
        if now < self.next_frame {
            std::thread::sleep(self.next_frame - now);
        }
        // Schedule the next deadline; if we fell behind (a slow frame), resync
        // from now so we don't try to "catch up" with a burst of zero-sleep
        // frames.
        self.next_frame += budget;
        let now = Instant::now();
        if self.next_frame < now {
            self.next_frame = now + budget;
        }
        Ok(())
    }
}
