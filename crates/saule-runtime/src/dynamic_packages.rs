//! Dynamically-loaded native packages.
//!
//! Where [`crate::native_packages`] handles packages that are *statically
//! linked* into the runtime (the stdlib), this module handles packages
//! that live **outside** the binary as shared libraries and are described by
//! a TOML manifest. The pipeline is:
//!
//! ```text
//! ~/.saule/native_manifests/<pkg>.toml   ── describes exports + symbol names
//! ~/.saule/native_packages/<pkg>.{dll,so,dylib}  ── the compiled code
//! ```
//!
//! `~/.saule` is the Saule home directory; set `SAULE_HOME` to relocate it.
//! See [`saule_home`] for the exact resolution order.
//!
//! 1. [`discover`] (run once from [`crate::init`]) scans the manifest
//!    directory, parses every `*.toml`, and records the resulting
//!    [`Manifest`]s in a process-global registry. **No binary is loaded yet.**
//! 2. [`register_sigs`] (driven by the typeck initializer, per thread) walks
//!    the discovered manifests and registers each method's type signature so
//!    `Graphics.circle(...)` type-checks *before* the binary is ever loaded.
//! 3. An `import X from "<pkg>"` resolves to a sentinel path
//!    ([`sentinel_path`]). The bytecode compiler resolves imports at
//!    *compile* time and folds a package's exports into constants, and a
//!    compile must never `dlopen` a library, so the step is split:
//!    * [`build_exports_deferred`] builds the package's surface from the
//!      manifest alone, each method resolving its symbol on first call.
//!      Nothing loads.
//!    * [`preload`] does the loading half — loads the shared library via
//!      `libloading` and checks every symbol the manifest names — and
//!      `run_program` calls it at *run* time, immediately before the body of
//!      the module that imported the package, so a package that fails to
//!      load fails at its `import`.
//!
//! The runtime never needs the package at compile time and the package
//! never links the runtime — the only shared contract is
//! [`saule_native_abi`].

mod bind;
mod discovery;
mod manifest;
#[cfg(test)]
mod tests;

pub use bind::*;
pub use discovery::*;
pub(crate) use manifest::*;

// Only the library-loading half of this module needs these; the manifest and
// type-signature half compiles on every target.
