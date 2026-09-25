//! `saule-engine-lib` — a Love2D-like graphics engine compiled as a Saule
//! *native package*, and the reference consumer of [`saule_sdk`].
//!
//! This crate is **not** linked into the interpreter. It is built as a
//! `cdylib` (`saule_engine_lib.dll` / `.so` / `.dylib`) and dropped into
//! `~/.saule/native_packages/`. That one file is the whole package: its
//! classes, every method's signature and doc comment are compiled into it,
//! and the interpreter reads them out of the file before it ever loads it.
//!
//! All of that — the `extern "C"` shims, argument decoding, error
//! marshalling, signatures and metadata — is handled by [`saule_sdk`]. Each
//! module exposes plain safe functions annotated with `#[saule_export]`; the
//! package itself is declared with [`saule_package!`](saule_sdk::saule_package)
//! below.
//!
//! ## Building
//!
//! ```text
//! cargo build -p saule-engine-lib --release
//! # then copy target/release/(lib)saule_engine_lib.{so,dylib,dll}
//! # into ~/.saule/native_packages/ — or run scripts/install_*.
//! ```

mod clipboard;
mod event;
mod font;
mod geom;
mod graphics;
mod image;
mod keyboard;
mod mouse;
mod raster;
mod render;
mod state;
mod timer;
mod window;

saule_sdk::saule_package! {
    name = "engine",
    version = "0.1.0",
    doc = "A Love2D-like 2D engine: a window, shapes, text, images and input.",
    classes {
        Graphics = "2D graphics: shapes, text, canvases, clipping, and transforms.",
        Keyboard = "Keyboard input: key state, per-frame press/release edges, and typed text.",
        Mouse = "Mouse input state.",
        Window = "Window management.",
        Timer = "Timing helpers.",
        Clipboard = "System clipboard: copy and paste plain text.",
    }
}

/// A global allocator that can be told to count allocations for a moment.
///
/// The renderer is meant to be allocation-free once its scratch buffers have
/// grown, and that is a property no timing benchmark can actually pin down — a
/// machine under load makes any wall-clock number arguable. Counting is exact:
/// arm the counter, draw a frame, and assert nothing was allocated.
///
/// The counters are **thread-local**, which is the part that makes the
/// measurement mean anything: the test harness runs tests in parallel, so a
/// process-wide counter measures whatever else happened to be running at the
/// same time. They are `const`-initialised `Cell`s so that reading them inside
/// the allocator cannot itself allocate and recurse.
#[cfg(test)]
mod counting_allocator {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ARMED: Cell<bool> = const { Cell::new(false) };
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    /// Record one allocation, if this thread is currently measuring.
    ///
    /// `try_with` rather than `with`: during thread teardown the local is gone,
    /// and an allocation then must not panic.
    fn tally() {
        let armed = ARMED.try_with(Cell::get).unwrap_or(false);
        if armed {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        }
    }

    pub struct Counting;

    // Safety: every method forwards to the system allocator unchanged; the
    // counter is incidental bookkeeping on the side.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            tally();
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            tally();
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    /// Run `f` with allocation counting on, and report how many it made on
    /// this thread.
    pub fn count(f: impl FnOnce()) -> usize {
        COUNT.with(|c| c.set(0));
        ARMED.with(|a| a.set(true));
        f();
        ARMED.with(|a| a.set(false));
        COUNT.with(Cell::get)
    }
}

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: counting_allocator::Counting = counting_allocator::Counting;

#[cfg(test)]
mod tests {
    #[test]
    fn graphics_circle_without_window_errors() {
        // No window has been created on this test thread, so drawing must
        // fail cleanly rather than crash.
        let result = super::graphics::graphics_circle("fill".to_string(), 100.0, 120.0, 50.0, None);
        assert!(result.is_err());
    }

    #[test]
    fn window_is_open_without_window_is_false() {
        // The loop condition is false when no window exists.
        assert!(!super::window::window_is_open());
    }

    #[test]
    fn keyboard_is_down_without_window_is_false() {
        // Keyboard polling degrades gracefully without an open window.
        assert!(!super::keyboard::keyboard_is_down("space".to_string()));
    }

    #[test]
    fn keyboard_unknown_key_is_false() {
        // Unrecognised key names silently return false, never panic.
        assert!(!super::keyboard::keyboard_is_down("hyperspace".to_string()));
    }

    #[test]
    fn mouse_is_down_without_window_is_false() {
        // Mouse polling degrades gracefully without an open window.
        assert!(!super::mouse::mouse_is_down(1));
    }
}
