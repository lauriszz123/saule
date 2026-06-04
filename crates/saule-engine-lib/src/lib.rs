//! `saule-engine-lib` — a Love2D-like graphics engine compiled as a Saule
//! *native package*.
//!
//! This crate is **not** linked into the interpreter. It is built as a
//! `cdylib` (`saule_engine_lib.dll` / `.so` / `.dylib`), dropped into
//! `~/.saule/native_packages/`, and described by a TOML manifest in
//! `~/.saule/native_manifests/`. At runtime the interpreter loads the
//! shared library and calls the `extern "C"` symbols named in the manifest.
//!
//! Every exported symbol follows the [`saule_native_abi`] calling
//! convention: `(args: *const CValue, argc: usize, out: *mut CValue) -> i32`.
//! See [`args`] for the small helper layer that makes reading arguments
//! ergonomic.
//!
//! ## Building
//!
//! ```text
//! cargo build -p saule-engine-lib --release
//! # then copy target/release/saule_engine_lib.{dll,so,dylib}
//! #   into ~/.saule/native_packages/ under the name the manifest expects.
//! ```

mod args;
mod graphics;
mod state;
mod timer;
mod window;

#[cfg(test)]
mod tests {
    use super::args::Args;
    use saule_native_abi::{tag, CValue};

    #[test]
    fn graphics_circle_without_window_errors() {
        // No window has been created on this test thread, so drawing must
        // fail cleanly rather than crash.
        let argv = [
            CValue::string_borrowed(b"fill"),
            CValue::float(100.0),
            CValue::float(120.0),
            CValue::float(50.0),
        ];
        let mut out = CValue::nil();
        let code = unsafe {
            super::graphics::saule_engine_graphics_circle(argv.as_ptr(), argv.len(), &mut out)
        };
        assert_ne!(code, 0);
        assert_eq!(out.tag, tag::ERR);
    }

    #[test]
    fn arity_mismatch_is_an_error() {
        let argv = [CValue::float(1.0)];
        let mut out = CValue::nil();
        let code = unsafe {
            super::window::saule_engine_window_create(argv.as_ptr(), argv.len(), &mut out)
        };
        assert_ne!(code, 0);
        assert_eq!(out.tag, tag::ERR);
    }

    #[test]
    fn args_wrapper_type_checks() {
        let argv = [CValue::integer(7)];
        let a = unsafe { Args::new(argv.as_ptr(), argv.len()) };
        assert_eq!(a.integer(0).unwrap(), 7);
        assert!(a.float(0).is_err());
    }
}
