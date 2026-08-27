//! `saule-engine-lib` — a Love2D-like graphics engine compiled as a Saule
//! *native package*, and the reference consumer of [`saule_sdk`].
//!
//! This crate is **not** linked into the interpreter. It is built as a
//! `cdylib` (`saule_engine_lib.dll` / `.so` / `.dylib`), dropped into
//! `~/.saule/native_packages/`, and described by a TOML manifest in
//! `~/.saule/native_manifests/`. At runtime the interpreter loads the shared
//! library and calls the `extern "C"` symbols named in the manifest.
//!
//! All of that ABI plumbing — the `extern "C"` shims, argument decoding,
//! error marshalling, signature strings, and manifest registration — is
//! handled by [`saule_sdk`]. Each module exposes plain safe functions
//! annotated with `#[saule_export]`; the package itself is declared with
//! [`saule_package!`](saule_sdk::saule_package) below. The `gen-manifest`
//! binary renders the manifest from these declarations.
//!
//! ## Building
//!
//! ```text
//! cargo build -p saule-engine-lib --release
//! # then install with scripts/install_wsl.sh (Linux/WSL) or copy the
//! # library + generated engine.toml into ~/.saule/ manually.
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
    binary = [
        "saule_engine_lib.so",
        "saule_engine_lib.dll",
        "saule_engine_lib.dylib",
    ],
    classes {
        Graphics = "2D graphics: shapes, text, canvases, clipping, and transforms.",
        Keyboard = "Keyboard input: key state, per-frame press/release edges, and typed text.",
        Mouse = "Mouse input state.",
        Window = "Window management.",
        Timer = "Timing helpers.",
        Clipboard = "System clipboard: copy and paste plain text.",
    }
}

/// Force the linker to retain every `#[saule_export]` object file when this
/// crate is consumed as an rlib by the `gen-manifest` binary.
///
/// Each `#[saule_export]` expands to a generated `extern "C"` shim plus an
/// `inventory::submit!` for that method. When a *separate* binary links this
/// crate as a static archive, the linker only pulls in archive members that
/// resolve a referenced symbol — so an unreferenced module's method
/// registration would be dropped and the manifest would come out incomplete.
/// Taking the address of each shim here references those members, keeping
/// their colocated registrations alive. `gen-manifest` calls this before
/// rendering. It is a no-op at runtime.
///
/// This list was maintained by hand and had drifted — Clipboard, the window
/// chrome methods, `newImage`, `drawFrame` and others were never listed, and
/// the manifest only came out complete because of how the compiler happened to
/// partition code into object files. `manifest_matches_the_checked_in_file` in
/// the tests below is the real guard now: it renders the manifest and compares
/// it against `engine.toml`, so an export that goes missing here fails the
/// build rather than silently disappearing from the package.
#[doc(hidden)]
pub fn anchor() {
    use std::hint::black_box;

    // Window
    black_box(window::saule_export_Window_create as *const ());
    black_box(window::saule_export_Window_setTitle as *const ());
    black_box(window::saule_export_Window_getPosition as *const ());
    black_box(window::saule_export_Window_setPosition as *const ());
    black_box(window::saule_export_Window_setTopmost as *const ());
    black_box(window::saule_export_Window_isFocused as *const ());
    black_box(window::saule_export_Window_getScale as *const ());
    black_box(window::saule_export_Window_isOpen as *const ());
    black_box(window::saule_export_Window_pollEvents as *const ());
    black_box(window::saule_export_Window_close as *const ());
    black_box(window::saule_export_Window_getSize as *const ());
    black_box(window::saule_export_Window_setQuitOnEscape as *const ());
    black_box(window::saule_export_Window_getQuitOnEscape as *const ());
    black_box(window::saule_export_Window_setTargetFPS as *const ());
    black_box(window::saule_export_Window_getTargetFPS as *const ());

    // Keyboard
    black_box(keyboard::saule_export_Keyboard_isDown as *const ());
    black_box(keyboard::saule_export_Keyboard_isAnyDown as *const ());
    black_box(keyboard::saule_export_Keyboard_getKeysDown as *const ());
    black_box(keyboard::saule_export_Keyboard_setKeyRepeat as *const ());
    black_box(keyboard::saule_export_Keyboard_hasKeyRepeat as *const ());
    black_box(keyboard::saule_export_Keyboard_setTextInput as *const ());
    black_box(keyboard::saule_export_Keyboard_hasTextInput as *const ());

    // Mouse
    black_box(mouse::saule_export_Mouse_getPos as *const ());
    black_box(mouse::saule_export_Mouse_isDown as *const ());
    black_box(mouse::saule_export_Mouse_setCursor as *const ());
    black_box(mouse::saule_export_Mouse_setVisible as *const ());

    // Graphics
    black_box(graphics::saule_export_Graphics_clear as *const ());
    black_box(graphics::saule_export_Graphics_present as *const ());
    black_box(graphics::saule_export_Graphics_rectangle as *const ());
    black_box(graphics::saule_export_Graphics_circle as *const ());
    black_box(graphics::saule_export_Graphics_ellipse as *const ());
    black_box(graphics::saule_export_Graphics_arc as *const ());
    black_box(graphics::saule_export_Graphics_polygon as *const ());
    black_box(graphics::saule_export_Graphics_line as *const ());
    black_box(graphics::saule_export_Graphics_polyline as *const ());
    black_box(graphics::saule_export_Graphics_points as *const ());
    black_box(graphics::saule_export_Graphics_point as *const ());
    black_box(graphics::saule_export_Graphics_print as *const ());
    black_box(graphics::saule_export_Graphics_printf as *const ());
    black_box(graphics::saule_export_Graphics_newFont as *const ());
    black_box(graphics::saule_export_Graphics_setNewFont as *const ());
    black_box(graphics::saule_export_Graphics_setFont as *const ());
    black_box(graphics::saule_export_Graphics_getFont as *const ());
    black_box(graphics::saule_export_Graphics_getFontHeight as *const ());
    black_box(graphics::saule_export_Graphics_getTextWidth as *const ());
    black_box(graphics::saule_export_Graphics_setColor as *const ());
    black_box(graphics::saule_export_Graphics_getColor as *const ());
    black_box(graphics::saule_export_Graphics_setBackgroundColor as *const ());
    black_box(graphics::saule_export_Graphics_getBackgroundColor as *const ());
    black_box(graphics::saule_export_Graphics_setLineWidth as *const ());
    black_box(graphics::saule_export_Graphics_getLineWidth as *const ());
    black_box(graphics::saule_export_Graphics_setLineStyle as *const ());
    black_box(graphics::saule_export_Graphics_getLineStyle as *const ());
    black_box(graphics::saule_export_Graphics_setLineJoin as *const ());
    black_box(graphics::saule_export_Graphics_getLineJoin as *const ());
    black_box(graphics::saule_export_Graphics_setBlendMode as *const ());
    black_box(graphics::saule_export_Graphics_getBlendMode as *const ());
    black_box(graphics::saule_export_Graphics_setDefaultFilter as *const ());
    black_box(graphics::saule_export_Graphics_getDefaultFilter as *const ());
    black_box(graphics::saule_export_Graphics_reset as *const ());
    black_box(graphics::saule_export_Graphics_setScissor as *const ());
    black_box(graphics::saule_export_Graphics_intersectScissor as *const ());
    black_box(graphics::saule_export_Graphics_getScissor as *const ());
    black_box(graphics::saule_export_Graphics_newCanvas as *const ());
    black_box(graphics::saule_export_Graphics_setCanvas as *const ());
    black_box(graphics::saule_export_Graphics_getCanvas as *const ());
    black_box(graphics::saule_export_Graphics_draw as *const ());
    black_box(graphics::saule_export_Graphics_newImage as *const ());
    black_box(graphics::saule_export_Graphics_imageSize as *const ());
    black_box(graphics::saule_export_Graphics_drawFrame as *const ());
    black_box(graphics::saule_export_Graphics_push as *const ());
    black_box(graphics::saule_export_Graphics_pop as *const ());
    black_box(graphics::saule_export_Graphics_origin as *const ());
    black_box(graphics::saule_export_Graphics_translate as *const ());
    black_box(graphics::saule_export_Graphics_scale as *const ());
    black_box(graphics::saule_export_Graphics_rotate as *const ());
    black_box(graphics::saule_export_Graphics_shear as *const ());
    black_box(graphics::saule_export_Graphics_applyTransform as *const ());
    black_box(graphics::saule_export_Graphics_replaceTransform as *const ());
    black_box(graphics::saule_export_Graphics_getStackDepth as *const ());
    black_box(graphics::saule_export_Graphics_transformPoint as *const ());
    black_box(graphics::saule_export_Graphics_inverseTransformPoint as *const ());
    black_box(graphics::saule_export_Graphics_getWidth as *const ());
    black_box(graphics::saule_export_Graphics_getHeight as *const ());
    black_box(graphics::saule_export_Graphics_getDimensions as *const ());
    black_box(graphics::saule_export_Graphics_getDPIScale as *const ());
    black_box(graphics::saule_export_Graphics_getPixelWidth as *const ());
    black_box(graphics::saule_export_Graphics_getPixelHeight as *const ());
    black_box(graphics::saule_export_Graphics_getPixelDimensions as *const ());
    black_box(graphics::saule_export_Graphics_release as *const ());
    black_box(graphics::saule_export_Graphics_releaseFont as *const ());
    black_box(graphics::saule_export_Graphics_getStats as *const ());
    black_box(graphics::saule_export_Graphics_loadImage as *const ());
    black_box(graphics::saule_export_Graphics_loadFont as *const ());
    black_box(graphics::saule_export_Graphics_newImageFromBase64 as *const ());
    black_box(graphics::saule_export_Graphics_saveImage as *const ());
    black_box(graphics::saule_export_Graphics_setLinearGradient as *const ());
    black_box(graphics::saule_export_Graphics_setRadialGradient as *const ());
    black_box(graphics::saule_export_Graphics_clearGradient as *const ());
    black_box(graphics::saule_export_Graphics_hasGradient as *const ());

    // Timer
    black_box(timer::saule_export_Timer_getTime as *const ());
    black_box(timer::saule_export_Timer_getDelta as *const ());
    black_box(timer::saule_export_Timer_getFPS as *const ());
    black_box(timer::saule_export_Timer_sleep as *const ());

    // Clipboard
    black_box(clipboard::saule_export_Clipboard_get as *const ());
    black_box(clipboard::saule_export_Clipboard_set as *const ());
    black_box(clipboard::saule_export_Clipboard_hasText as *const ());

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
    /// The manifest the interpreter loads must describe the exports this crate
    /// actually has.
    ///
    /// `engine.toml` is checked in and the Unix install scripts copy it
    /// verbatim, so a signature changed without regenerating it produced a
    /// package whose declared types disagreed with its code — a mismatch that
    /// only showed up as a confusing runtime error in somebody's `.sau`
    /// program. Nothing checked for it before; this does, on every test run.
    #[test]
    fn manifest_matches_the_checked_in_file() {
        super::anchor();

        let rendered = saule_sdk::manifest::render().expect("render the manifest");
        let checked_in = include_str!("../engine.toml");

        assert_eq!(
            rendered.trim(),
            checked_in.trim(),
            "engine.toml is stale — regenerate it with:\n    \
             cargo run --release -p saule-engine-lib --bin gen-manifest -- \
             crates/saule-engine-lib/engine.toml"
        );
    }

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
