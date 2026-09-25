//! Dynamically-loaded native packages.
//!
//! Where [`crate::native_packages`] handles packages that are *statically
//! linked* into the runtime (the stdlib), this module handles packages
//! that live **outside** the binary as shared libraries. A package is one
//! file, which describes itself: its classes, enums, every member's
//! signature and doc comment are metadata records compiled into the library
//! by `saule-sdk` (see `saule_native_abi`, "Package metadata").
//!
//! ```text
//! ~/.saule/native_packages/<pkg>.{dll,so,dylib}   ── the package, whole
//! ```
//!
//! `~/.saule` is the Saule home directory; set `SAULE_HOME` to relocate it.
//! See [`saule_home`] for the exact resolution order.
//!
//! 1. [`discover`] (run once from [`crate::init`]) scans the packages
//!    directory and reads each library's metadata **out of the file** —
//!    [`embedded`] parses it as data; nothing is loaded — into a [`Manifest`]
//!    in a process-global registry. A library that cannot be used is
//!    remembered with the reason ([`rejection`]), which the `import` naming
//!    it reports.
//! 2. [`register_sigs`] (driven by the typeck initializer, per thread)
//!    registers each static member's type signature, and [`seed_classes`] /
//!    [`seed_enums`] give the checker each class and enum an import brings in,
//!    so `Graphics.circle(…)`, `Image(…)` and `img.width` type-check *before*
//!    the binary is ever loaded.
//! 3. An `import X from "<pkg>"` resolves to a sentinel path
//!    ([`sentinel_path`]). The bytecode compiler resolves imports at
//!    *compile* time and folds a package's exports into constants, and a
//!    compile must never `dlopen` a library, so the step is split:
//!    * [`build_exports_deferred`] builds the package's surface from its
//!      metadata alone, each member resolving its symbol on first call.
//!      Nothing loads.
//!    * [`preload`] does the loading half — loads the shared library via
//!      `libloading`, checks its ABI version, and resolves every symbol the
//!      metadata names — and `run_program` calls it at *run* time,
//!      immediately before the body of the module that imported the package,
//!      so a package that fails to load fails at its `import`.
//!
//! The runtime never needs the package at compile time and the package
//! never links the runtime — the only shared contract is
//! [`saule_native_abi`].

mod bind;
mod discovery;
mod embedded;
mod manifest;
#[cfg(test)]
mod tests;

pub use bind::*;
pub use discovery::*;
pub(crate) use manifest::*;
