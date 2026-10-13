//! `saule-sdk` — write a Saule native package in ordinary Rust.
//!
//! A native package is a `cdylib` the Saule interpreter loads at runtime.
//! Everything Saule needs to know about it — its classes, every method's
//! signature, the doc comments the editor shows — is compiled *into the
//! library* by the macros here, so the one file you build is the whole
//! package. There is no manifest to generate or keep in sync.
//!
//! ```ignore
//! use saule_sdk::prelude::*;
//!
//! saule_package! {
//!     name = "gfx",
//!     version = "0.1.0",
//!     classes { Gfx = "Loading and saving images." }
//! }
//!
//! /// An RGBA image held in memory.
//! #[saule_class]
//! pub struct Image { w: i64, h: i64, pixels: Vec<u32> }
//!
//! #[saule_methods]
//! impl Image {
//!     /// A blank image: `Image(64, 64)` in Saule.
//!     pub fn new(w: i64, h: i64) -> Self {
//!         Image { w, h, pixels: vec![0; (w * h) as usize] }
//!     }
//!     /// Width in pixels, read as `img.width`.
//!     #[saule(getter)]
//!     pub fn width(&self) -> i64 { self.w }
//!     /// Paint every pixel: `img.fill(0xff0000ff)`.
//!     pub fn fill(&mut self, color: i64) { self.pixels.fill(color as u32) }
//! }
//!
//! /// Decode a PNG: `Gfx.load("a.png")` returns an `Image`.
//! #[saule_export(class = "Gfx")]
//! fn load(path: &str) -> Result<Image, String> { /* … */ }
//! ```
//!
//! Saule then sees `Image` as a real class: `local img: Image = Gfx.load(p)`
//! type-checks, `img.fill("red")` does not, `img.` completes in the editor,
//! and each `///` comment shows on hover. When the last Saule value holding
//! an `Image` goes away, the `Image` is dropped.
//!
//! ## The pieces
//!
//! ## Operators and interfaces
//!
//! A `#[saule_class]` may declare the interfaces it satisfies, including the
//! built-in operator contracts, so `a + b` works on a native object exactly
//! as it does on a Saule class:
//!
//! ```ignore
//! #[saule_class(implements(OpAdd<Money, Money>, OpEq<Money>, OpToString))]
//! pub struct Money { cents: i64 }
//!
//! #[saule_methods]
//! impl Money {
//!     pub fn add(&self, other: &Money) -> Money { /* `a + b` */ }
//!     pub fn equals(&self, other: &Money) -> bool { /* `a == b` */ }
//!     #[saule(name = "toString")]
//!     pub fn render(&self) -> String { /* `tostring(a)` */ }
//! }
//! ```
//!
//! Each contract is satisfied by a method of the conventional name — `OpAdd`
//! by `add`, `OpNeg` by `neg`, `OpLen` by `len`, `OpCompare` by `compare`.
//! The list is compiled into the package description, so the checker sees it
//! without loading the library and `a + b` type-checks before anything binds.
//! Declaring a contract whose method is missing is an error at the use site
//! rather than a surprise at run time.
//!
//! Two things to know. `OpToString` wants a Saule method named `toString`,
//! and an inherent Rust `to_string` shadows `Display`'s, which clippy
//! rejects — so name it something else and map it with
//! `#[saule(name = "toString")]`. And a contract's type arguments are written
//! as they would be in Saule: `implements(OpAdd<Money, Money>)`, not Rust
//! turbofish.
//!
//! - [`saule_package!`] — the package's name and version. Exactly one.
//! - [`#[saule_export]`](saule_export) — a free function as a static member
//!   of a class (`Gfx.load`). Great for namespaces of functions.
//! - [`#[saule_class]`](saule_class) + [`#[saule_methods]`](saule_methods) —
//!   a struct as a class with a constructor, methods, static functions and
//!   properties.
//! - [`#[saule_enum]`](saule_enum) — a fieldless enum as a Saule enum.
//! - [`SObject<T>`] — a shared handle to an object, to keep one beyond a
//!   call or return one you also keep.
//! - The `S*` [`types`] — Saule tables and callbacks, worked on in place.
//!
//! See [`convert`] for how Rust types map to Saule types. A function may
//! return `T`, `()`, `Result<T, E>` (where `E: Display`; an `Err` becomes a
//! Saule runtime error at the call site), or a tuple `(A, B, …)` for a
//! multi-value return. A panic becomes a runtime error too.

pub mod convert;
pub mod host;
pub mod object;
pub mod types;

pub use object::{NativeClass, SAnyObject, SObject};
pub use saule_export_macro::{saule_class, saule_enum, saule_export, saule_methods, saule_package};

/// Everything a package usually needs, in one import.
pub mod prelude {
    pub use crate::convert::{FromSaule, IntoSaule};
    pub use crate::object::{NativeClass, SAnyObject, SObject};
    pub use crate::types::{
        SBool, SElem, SFloat, SFunction, SInteger, SString, STable, SValue, T, U, Untyped, V, W,
    };
    pub use saule_export_macro::{
        saule_class, saule_enum, saule_export, saule_methods, saule_package,
    };
}

/// Implementation details referenced by generated code. Not a stable API —
/// do not use these paths directly.
#[doc(hidden)]
pub mod __private {
    pub use crate::convert::{FromSaule, IntoSaule, run_export};
    pub use crate::host::__set_host;
    pub use crate::object::{ClassDescriptor, release_object};
    pub use saule_native_abi::{
        ABI_VERSION, CValue, HostApi, NativeSymbolFn, ObjectPtr, return_error,
    };
}
