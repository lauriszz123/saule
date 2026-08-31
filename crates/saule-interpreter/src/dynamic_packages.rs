//! Dynamically-loaded native packages.
//!
//! Where [`crate::native_packages`] handles packages that are *statically
//! linked* into the interpreter (the stdlib), this module handles packages
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
//! 3. On the first `import X from "<pkg>"`, the module loader resolves the
//!    import to a sentinel path ([`sentinel_path`]) and calls
//!    [`build_exports`], which lazily loads the shared library via
//!    `libloading`, resolves the symbols named in the manifest, and wraps
//!    each one in a [`Value::NativeClosure`].
//!
//! The interpreter never needs the package at compile time and the package
//! never links the interpreter — the only shared contract is
//! [`saule_native_abi`].
//!
//! ## The bytecode compiler's route through the same pipeline
//!
//! `saule-vm` resolves imports at *compile* time and folds a package's
//! exports into constants, so it cannot use step 3 — that would `dlopen` a
//! library while compiling, which a compile must never do. It splits the
//! step instead:
//!
//! * [`build_exports_deferred`] builds the same surface from the manifest
//!   alone, each method resolving its symbol on first call. Nothing loads.
//! * [`preload`] does the loading half, and `run_program` calls it at *run*
//!   time, immediately before the body of the module that imported the
//!   package — the same point step 3 reaches under the tree-walker, so a
//!   package that fails to load fails identically under both engines.

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
