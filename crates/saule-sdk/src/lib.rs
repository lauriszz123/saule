//! `saule-sdk` — build Saule native packages without writing ABI boilerplate.
//!
//! A native package is a `cdylib` the Saule interpreter loads at runtime. The
//! interpreter calls `extern "C"` symbols across a small, frozen C ABI
//! ([`saule_native_abi`]) and discovers them through a TOML manifest. Writing
//! those shims and the manifest by hand is repetitive and easy to get out of
//! sync — this SDK does that heavy lifting so you write only safe Rust:
//!
//! ```ignore
//! use saule_sdk::prelude::*;
//!
//! saule_package! {
//!     name = "engine",
//!     version = "0.1.0",
//!     binary = ["saule_engine_lib.so", "saule_engine_lib.dll", "saule_engine_lib.dylib"],
//!     classes {
//!         Window = "Window management.",
//!     }
//! }
//!
//! #[saule_export(class = "Window", name = "create")]
//! fn window_create(width: i64, height: i64, title: Option<String>) -> Result<(), String> {
//!     // ... your logic; no `unsafe`, no CValue, no manifest entry ...
//!     Ok(())
//! }
//! ```
//!
//! The `#[saule_export]` macro:
//! - generates the `extern "C"` shim (all the `unsafe` pointer handling,
//!   arity checking, argument decoding, and error/return marshalling),
//! - **infers the Saule signature from the Rust types** (so the manifest can
//!   never drift), and
//! - registers the method in the manifest registry.
//!
//! A small generator binary then calls [`manifest::render`] to emit the
//! `<package>.toml`. See the `saule-engine-lib` crate for a complete example.
//!
//! ## Type mapping
//!
//! See [`convert`] for the supported Rust ⇄ Saule type mappings. A function
//! may return `T`, `()`, `Result<T, E>` (where `E: Display`; an `Err` becomes
//! a Saule runtime error at the call site), or a tuple `(A, B, …)` for a
//! multi-value return that the caller can destructure
//! (`local a, b = Class.method()`).

pub mod convert;
pub mod host;
pub mod manifest;
pub mod types;

/// Re-export of the `#[saule_export]` attribute macro.
pub use saule_export_macro::saule_export;

/// The common imports for writing a package: the export macro, the
/// `saule_package!` declaration macro, the conversion traits, and the
/// Saule-typed `S*` bridge types.
pub mod prelude {
    pub use crate::convert::{FromSaule, IntoSaule};
    pub use crate::saule_export;
    pub use crate::saule_package;
    pub use crate::types::{
        SBool, SElem, SFloat, SFunction, SInteger, SString, STable, SValue, T, U, Untyped, V, W,
    };
}

/// Implementation details referenced by the generated code from
/// `#[saule_export]` and `saule_package!`. Not a stable API — do not use
/// these paths directly.
#[doc(hidden)]
pub mod __private {
    pub use crate::convert::{FromSaule, IntoSaule};
    pub use crate::host::__set_host;
    pub use crate::manifest::{ExportedClass, ExportedMethod, PackageInfo};
    pub use inventory;
    pub use saule_native_abi::{CValue, HostApi, NativeSymbolFn, return_error};
}

/// Declare the package and its classes for manifest generation.
///
/// Registers a single [`PackageInfo`](manifest::PackageInfo) and one
/// [`ExportedClass`](manifest::ExportedClass) per class. Methods are
/// registered separately by `#[saule_export]`.
///
/// ```ignore
/// saule_package! {
///     name = "engine",
///     version = "0.1.0",
///     binary = ["saule_engine_lib.so", "saule_engine_lib.dll", "saule_engine_lib.dylib"],
///     classes {
///         Graphics = "2D graphics rendering.",
///         Window   = "Window management.",
///     }
/// }
/// ```
#[macro_export]
macro_rules! saule_package {
    (
        name = $name:literal,
        version = $version:literal,
        binary = [ $($bin:literal),+ $(,)? ],
        classes { $( $class:ident = $doc:literal ),* $(,)? } $(,)?
    ) => {
        $crate::__private::inventory::submit! {
            $crate::__private::PackageInfo {
                name: $name,
                version: $version,
                binary: &[ $($bin),+ ],
            }
        }
        $(
            $crate::__private::inventory::submit! {
                $crate::__private::ExportedClass {
                    name: ::core::stringify!($class),
                    doc: $doc,
                }
            }
        )*

        // Receive the host callback table so `STable` / `SFunction` can
        // operate on host-owned reference values. The interpreter calls this
        // right after loading the library; packages that only use scalars
        // simply never touch the stored pointer.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn saule_set_host(
            api: *const $crate::__private::HostApi,
        ) {
            unsafe { $crate::__private::__set_host(api) };
        }
    };
}
