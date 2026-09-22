//! The Saule runtime: what a running program is made of.
//!
//! `saule-vm` compiles a program to bytecode and executes it; everything the
//! executing program *touches* lives here — its values, the standard
//! library, native packages, and the handful of operations that are decided
//! by the value in hand rather than at compile time.
//!
//! ## Pipeline
//!
//! ```text
//! source ─► saule_lexer ─► saule_parser ─► saule_semantic ─► saule_typeck ─► saule-vm
//!                                          (registry,         (types,          (compile,
//!                                           definite assn.,    nullability,     run on
//!                                           control flow)      match exhaust.)  this crate)
//! ```
//!
//! Runtime errors are kept disjoint from compile-time ones — see
//! [`RuntimeError`] for the genuinely-dynamic failures that can still fire
//! while a program runs (division by zero, force-unwrap of `nil`, uncaught
//! `throw`, file-I/O errors, …).
//!
//! ## Module layout
//!
//! | Module       | Responsibility                                          |
//! |--------------|---------------------------------------------------------|
//! | [`value`]    | Runtime [`Value`] enum and the objects behind it        |
//! | [`prelude`]  | The names every program starts with ([`Prelude`])       |
//! | [`stdlib`]   | Standard library, installed into the prelude            |
//! | [`ops`]      | Operators on values the compiler could not type         |
//! | [`cast`]     | `x as T`: the checked downcast and the conversion       |
//! | [`members`]  | `obj.name` and `obj[index]` by name, read and written   |
//! | [`call`]     | Calling a value or a method by name; the depth guard    |
//! | [`module`]   | Where an `import` points; the typecheck seed            |
//! | [`error`]    | [`RuntimeError`] (miette-aware diagnostics)             |
//! | [`semantic`] | Re-export of `saule-semantic`                           |
//! | [`typeck`]   | Re-export of `saule-typeck`                             |

use saule_ast::Module;

pub mod call;
pub mod cast;
pub mod dynamic_packages;
pub mod error;
pub mod fxhash;
pub mod gc;
mod index_hooks;
pub mod itoa;
pub mod members;
pub mod module;
// The host-callback table exists solely so a dlopen'd package can manipulate
// host-owned values by handle. Without `native-packages` there is nothing to
// hand it to, so the whole module goes away rather than sitting dead.
#[cfg(feature = "native-packages")]
mod native_host;
pub mod native_packages;
pub mod ops;
pub mod output;
pub mod platform;
pub mod prelude;
pub mod stdlib;
pub mod value;

/// Re-export of the standalone `saule-typeck` crate.
pub use saule_typeck as typeck;

/// Re-export of `saule-project`, which owns `saule.config` and the
/// [`project::ProjectInfo`] import resolution consults. The runtime does not
/// read the config itself — it is handed the resolved project by whoever is
/// driving it — but it does define the shape of the answer.
pub use saule_project as project;

/// Re-export of the `saule-semantic` crate. The standard pipeline runs
/// `semantic::analyze` before `typeck::check`.
pub use saule_semantic as semantic;

pub use call::{DepthGuard, enter_call_depth};
pub use error::RuntimeError;
pub use prelude::Prelude;
pub use value::{NativeFn, Value};

/// Unified diagnostic for the static half of the pipeline. Each variant is
/// `#[diagnostic(transparent)]` so miette renders it indistinguishably from
/// the inner family's own diagnostics.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum PipelineError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Semantic(#[from] saule_semantic::SemanticError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Typeck(#[from] saule_typeck::TypeCheckError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Runtime(#[from] RuntimeError),
}

/// One-shot, idempotent initialization every embedder should call before any
/// `typeck::check` or run: wires the stdlib's native signatures into the
/// typechecker's registry and its names into the resolver's prelude.
///
/// Called by every pipeline helper here and by `saule-vm`'s entry points, so
/// embedders going through those don't need to call it themselves.
/// Standalone users of `saule_runtime::typeck::check` should call it once at
/// startup.
pub fn init() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        saule_typeck::sigs::set_initializer(stdlib::register_all_sigs);
        saule_semantic::prelude::set_provider(stdlib::all_prelude_names);
        saule_semantic::builtins::set_provider(stdlib::builtin_registries);
        // Built-in native packages — anything we ship with the runtime that
        // lives behind an `import "..."`. Third-party packages call
        // `native_packages::register` themselves.
        stdlib::register_builtin_packages();
        // Discover externally-installed native packages described by
        // manifests under `~/.saule/native_manifests/`. This only parses
        // manifests and records their type signatures — the shared
        // libraries themselves are loaded lazily on first import.
        dynamic_packages::discover();
    });
}

/// Run `saule_semantic`'s analysis, handing back both its diagnostics and
/// the binding table the compiler reads.
pub fn analyze_with_bindings(
    module: &Module,
    seed: semantic::ModuleSeed,
) -> (Vec<semantic::SemanticError>, semantic::Bindings) {
    init();
    semantic::analyze_with_bindings(module, seed)
}

/// The whole static half of the pipeline, in the one order the passes can
/// run in: semantic analysis, then typecheck-and-resolve. Typecheck needs
/// the registries semantic analysis installs, so it cannot come first.
///
/// Returns the first error from whichever pass failed; a module that errors
/// is never compiled, so later passes are skipped.
pub fn analyze_and_check(
    module: &mut Module,
    seed: semantic::ModuleSeed,
) -> Result<(), PipelineError> {
    let (errors, _) = analyze_with_bindings(module, seed);
    if let Some(first) = errors.into_iter().next() {
        return Err(PipelineError::Semantic(first));
    }
    if let Some(first) = typeck::check_and_resolve(module).into_iter().next() {
        return Err(PipelineError::Typeck(first));
    }
    Ok(())
}
