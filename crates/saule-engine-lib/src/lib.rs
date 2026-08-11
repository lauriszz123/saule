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
#[doc(hidden)]
pub fn anchor() {
    use std::hint::black_box;

    // Window exports
    black_box(window::saule_export_Window_create as *const ());
    black_box(window::saule_export_Window_isOpen as *const ());
    black_box(window::saule_export_Window_pollEvents as *const ());
    black_box(window::saule_export_Window_close as *const ());
    black_box(window::saule_export_Window_getSize as *const ());

    // Keyboard exports
    black_box(keyboard::saule_export_Keyboard_isDown as *const ());
    black_box(keyboard::saule_export_Keyboard_isAnyDown as *const ());
    black_box(keyboard::saule_export_Keyboard_getKeysDown as *const ());
    black_box(keyboard::saule_export_Keyboard_setKeyRepeat as *const ());
    black_box(keyboard::saule_export_Keyboard_hasKeyRepeat as *const ());
    black_box(keyboard::saule_export_Keyboard_setTextInput as *const ());
    black_box(keyboard::saule_export_Keyboard_hasTextInput as *const ());

    // Mouse exports
    black_box(mouse::saule_export_Mouse_isDown as *const ());
    black_box(mouse::saule_export_Mouse_getPos as *const ());

    // Graphics — frame lifecycle
    black_box(graphics::saule_export_Graphics_clear as *const ());
    black_box(graphics::saule_export_Graphics_present as *const ());

    // Graphics — shapes
    black_box(graphics::saule_export_Graphics_rectangle as *const ());
    black_box(graphics::saule_export_Graphics_circle as *const ());
    black_box(graphics::saule_export_Graphics_ellipse as *const ());
    black_box(graphics::saule_export_Graphics_arc as *const ());
    black_box(graphics::saule_export_Graphics_polygon as *const ());
    black_box(graphics::saule_export_Graphics_line as *const ());
    black_box(graphics::saule_export_Graphics_polyline as *const ());
    black_box(graphics::saule_export_Graphics_points as *const ());
    black_box(graphics::saule_export_Graphics_point as *const ());

    // Graphics — text
    black_box(graphics::saule_export_Graphics_print as *const ());
    black_box(graphics::saule_export_Graphics_printf as *const ());
    black_box(graphics::saule_export_Graphics_newFont as *const ());
    black_box(graphics::saule_export_Graphics_setNewFont as *const ());
    black_box(graphics::saule_export_Graphics_setFont as *const ());
    black_box(graphics::saule_export_Graphics_getFont as *const ());
    black_box(graphics::saule_export_Graphics_getFontHeight as *const ());
    black_box(graphics::saule_export_Graphics_getTextWidth as *const ());

    // Graphics — colour, lines, blending
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

    // Graphics — clipping
    black_box(graphics::saule_export_Graphics_setScissor as *const ());
    black_box(graphics::saule_export_Graphics_intersectScissor as *const ());
    black_box(graphics::saule_export_Graphics_getScissor as *const ());

    // Graphics — canvases
    black_box(graphics::saule_export_Graphics_newCanvas as *const ());
    black_box(graphics::saule_export_Graphics_setCanvas as *const ());
    black_box(graphics::saule_export_Graphics_getCanvas as *const ());
    black_box(graphics::saule_export_Graphics_draw as *const ());

    // Graphics — coordinate system
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

    // Graphics — dimensions
    black_box(graphics::saule_export_Graphics_getWidth as *const ());
    black_box(graphics::saule_export_Graphics_getHeight as *const ());
    black_box(graphics::saule_export_Graphics_getDimensions as *const ());
    black_box(graphics::saule_export_Graphics_getDPIScale as *const ());
    black_box(graphics::saule_export_Graphics_getPixelWidth as *const ());
    black_box(graphics::saule_export_Graphics_getPixelHeight as *const ());
    black_box(graphics::saule_export_Graphics_getPixelDimensions as *const ());

    // Timer exports
    black_box(timer::saule_export_Timer_getTime as *const ());
    black_box(timer::saule_export_Timer_getDelta as *const ());
}

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
