//! Tree-walking interpreter for the Saule language.
//!
//! ## Pipeline
//!
//! ```text
//! source ─► saule_lexer ─► saule_parser ─► saule_semantic ─► saule_typeck ─► run / run_in
//!                                          (registry,         (types,
//!                                           definite assn.,    nullability,
//!                                           control flow)      match exhaust.)
//! ```
//!
//! The CLI and the module loader walk that pipeline in order: a non-empty
//! semantic-error list means typecheck is skipped, and a non-empty typecheck
//! error list means execution is skipped. Runtime errors are kept disjoint
//! from compile-time ones — see [`RuntimeError`] for the genuinely-dynamic
//! failures that can still fire during evaluation (division by zero,
//! force-unwrap of `nil`, uncaught `throw`, file-I/O errors, …).
//!
//! ## Module layout
//!
//! | Module       | Responsibility                              |
//! |--------------|---------------------------------------------|
//! | [`value`]    | Runtime [`Value`] enum and `NativeFn`       |
//! | [`env`]      | Lexical scopes ([`Environment`])            |
//! | [`stdlib`]   | Standard library installed into the prelude |
//! | [`error`]    | [`RuntimeError`] (miette-aware diagnostics) |
//! | [`eval`]     | Statement & expression evaluation           |
//! | [`module`]   | `import` loader — runs the full pipeline    |
//! | [`semantic`] | Re-export of `saule-semantic`               |
//! | [`typeck`]   | Re-export of `saule-typeck`                 |

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::Module;

pub(crate) mod capture;
pub mod dynamic_packages;
pub mod env;
pub mod error;
pub mod eval;
pub mod fxhash;
pub mod module;
// The host-callback table exists solely so a dlopen'd package can manipulate
// host-owned values by handle. Without `native-packages` there is nothing to
// hand it to, so the whole module goes away rather than sitting dead.
#[cfg(feature = "native-packages")]
mod native_host;
pub mod native_packages;
pub mod output;
pub mod platform;
pub(crate) mod recycle;
pub mod stdlib;
pub mod value;

/// Re-export of the standalone `saule-typeck` crate so existing call sites
/// (`saule_interpreter::typeck::check`) keep working after the extraction.
pub use saule_typeck as typeck;

/// Re-export of `saule-project`, which owns `saule.config` and the
/// [`project::ProjectInfo`] import resolution consults. The interpreter does
/// not read the config itself — it is handed the resolved project by whoever
/// is driving it — but it does define the shape of the answer, so the type
/// travels with the pipeline rather than with the CLI.
pub use saule_project as project;

/// Re-export of the `saule-semantic` crate. The standard pipeline runs
/// `semantic::analyze` before `typeck::check`, and both are then followed
/// by [`run_in`].
pub use saule_semantic as semantic;

pub use env::Environment;
pub use error::RuntimeError;
pub use eval::{DepthGuard, Flow, enter_call_depth};

/// Read `receiver.name` with the tree-walker's own member rules.
///
/// The dynamic form of a member access, for the bytecode compiler's `GETFX`
/// — the case where the front end did not prove the receiver's class and no
/// field slot could be resolved. Reused rather than reimplemented, the same
/// way `ARITHX` calls `ops::binary`: instance fields, methods, statics,
/// enum variants, `.name` and `.value`, and every error message come out
/// identical by construction.
pub fn read_member_dynamic(
    receiver: &Value,
    name: &str,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    eval::expr::members::read_member(receiver, name, span)
}

/// Call `receiver.name(args)` with the tree-walker's own dispatch.
///
/// The dynamic form of a method call. Covers every receiver kind in one
/// place — user instances, classes, enums, file handles, stdlib values —
/// which is exactly why it is worth reusing: the alternative is the
/// compiler learning each of them separately and diverging on the ones it
/// gets wrong.
pub fn call_member_dynamic(
    receiver: &Value,
    name: &str,
    args: &[Value],
    span: std::ops::Range<usize>,
) -> Result<Vec<Value>, RuntimeError> {
    let evaled: Vec<eval::expr::EvaluatedArg> = args
        .iter()
        .cloned()
        .map(eval::expr::EvaluatedArg::Positional)
        .collect();
    eval::expr::invoke_method_multi(receiver, name, evaled, span)
}
pub use value::{NativeFn, Value};

/// Unified diagnostic for the full source-to-value pipeline. Each variant
/// is `#[diagnostic(transparent)]` so miette renders it indistinguishably
/// from the inner family's own diagnostics — callers (CLI, tests) only need
/// to handle one error type.
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

/// One-shot, idempotent initialization the embedder should call before any
/// `typeck::check` / `run` pass. Today this just wires the stdlib's native
/// signatures into the typechecker's registry; future startup work (logging,
/// allocator hooks, …) can hang off the same hook.
///
/// Called automatically from [`run`], [`run_in`], [`call_function_value`],
/// [`call_class_static_method`], and from the module loader, so embedders
/// that go through those entry points don't need to invoke it explicitly.
/// Standalone users of `saule_interpreter::typeck::check` should call it
/// once at startup.
pub fn init() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        saule_typeck::sigs::set_initializer(stdlib::register_all_sigs);
        saule_semantic::prelude::set_provider(stdlib::all_prelude_names);
        saule_semantic::builtins::set_provider(stdlib::builtin_registries);
        // Built-in native packages — anything we ship with the
        // interpreter that lives behind an `import "..."`. Third-party
        // packages call `native_packages::register` themselves.
        stdlib::register_builtin_packages();
        // Discover externally-installed native packages described by
        // manifests under `~/.saule/native_manifests/`. This only parses
        // manifests and records their type signatures — the shared
        // libraries themselves are loaded lazily on first import.
        dynamic_packages::discover();
    });
}

/// Invoke a [`value::FunctionObject`] with the given arguments. Exposed so
/// embedders (the CLI, the REPL, future test runners) can call functions
/// that were defined in user code without re-parsing.
pub fn call_function_value(
    f: &std::rc::Rc<value::FunctionObject>,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    init();
    let evaled: Vec<eval::expr::EvaluatedArg> = args
        .iter()
        .cloned()
        .map(eval::expr::EvaluatedArg::Positional)
        .collect();
    eval::expr::call_function(f, &evaled, 0..0)
}

/// Invoke a static method on a class, with `self` bound to the class — the
/// CLI uses this to run `Main.main()`.
pub fn call_class_static_method(
    class: &std::rc::Rc<value::ClassObject>,
    method: &str,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    init();
    let f = class
        .lookup_static_method(method)
        .ok_or_else(|| RuntimeError::TypeError {
            message: format!("no static method `{method}` on class `{}`", class.name),
            span: 0..0,
        })?;
    let evaled: Vec<eval::expr::EvaluatedArg> = args
        .iter()
        .cloned()
        .map(eval::expr::EvaluatedArg::Positional)
        .collect();
    eval::expr::call_static_method_ref_multi(&f, class, &evaled, 0..0)
        .map(|vs| vs.into_iter().next().unwrap_or(Value::Nil))
}

/// Run a parsed [`Module`] in a fresh environment seeded with built-ins.
///
/// **Low-level entry point** — see [`check_and_run`] for the full pipeline
/// that runs `saule_semantic::analyze` and `saule_typeck::check` first.
///
/// Returns the value of the last evaluated expression-statement (useful for
/// the REPL and for tests).
pub fn run(module: &Module) -> Result<Value, RuntimeError> {
    init();
    let env = Environment::with_prelude();
    run_in(module, &env)
}

/// Run the full source-to-value pipeline on a parsed module:
/// `semantic::analyze` → `typeck::check` → [`run`]. Returns the first
/// diagnostic from whichever phase failed.
///
/// This is the entry point most embedders should use; the CLI's
/// `run_source` does the same thing but also carries `NamedSource` so
/// miette can render snippets.
/// Run `saule_semantic`'s analysis and publish what it learned about closure
/// capture, so lambdas in `module` capture exactly the bindings their bodies
/// refer to.
///
/// **Any embedder that analyses a module and then executes it should call
/// this rather than `semantic::analyze_with_seed`.** Skipping it is safe —
/// lambdas fall back to capturing their whole defining scope, which is what
/// they did before the analysis existed — but that fallback is the leak
/// described in `crate::capture`.
pub fn analyze_and_prepare(
    module: &Module,
    seed: semantic::ModuleSeed,
) -> Vec<semantic::SemanticError> {
    init();
    let (errors, bindings) = semantic::analyze_with_bindings(module, seed);
    capture::register(module, &bindings);
    errors
}

pub fn check_and_run(module: &Module) -> Result<Value, PipelineError> {
    check_and_run_in(module, None)
}

/// Like [`check_and_run`] but lets the caller specify the directory the
/// module lives in so cross-module imports can be resolved when building
/// the typecheck seed. Pass `None` for in-memory snippets (tests, REPL).
pub fn check_and_run_in(
    module: &Module,
    dir: Option<&std::path::Path>,
) -> Result<Value, PipelineError> {
    init();
    let seed = match dir {
        Some(d) => module::collect_import_seed(module, d),
        None => saule_semantic::ModuleSeed::default(),
    };
    if let Some(first) = analyze_and_prepare(module, seed).into_iter().next() {
        return Err(PipelineError::Semantic(first));
    }
    if let Some(first) = typeck::check(module).into_iter().next() {
        return Err(PipelineError::Typeck(first));
    }
    Ok(run(module)?)
}

/// Run a [`Module`] inside a caller-supplied environment.
///
/// **Low-level entry point** — assumes the caller has already invoked
/// `saule_semantic::analyze` and `saule_typeck::check` on the module. For
/// the safe, full-pipeline alternative see [`check_and_run`].
pub fn run_in(module: &Module, env: &Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
    init();
    match eval::stmt::exec_block(&module.stmts, env)? {
        Flow::Normal(v) => Ok(v),
        Flow::Return(values) => Ok(values.into_iter().next().unwrap_or(Value::Nil)),
        // A tail call must never outlive the body that produced one, and a
        // module body is not a function — there is no frame to replace. Make
        // the call for real. Like the arms below this is only reachable
        // through a bare `run_in` on a module `saule_semantic` never saw.
        Flow::TailCall { callee, args, span } => Ok(eval::expr::call_function(
            &callee, &args, span,
        )?),
        // `break` / `continue` / loose `return` at module top level are
        // rejected by `saule_semantic`'s control-flow walker before we ever
        // get here. The standard pipeline (CLI, module loader, and
        // [`check_and_run`]) guarantees that; only a bare `run_in` on an
        // unchecked module could reach these arms.
        Flow::Break => Err(RuntimeError::TypeError {
            message: "internal: `break` at module top level reached evaluation — \
                      `saule_semantic::analyze` was not run on this module"
                .to_string(),
            span: 0..0,
        }),
        Flow::Continue => Err(RuntimeError::TypeError {
            message: "internal: `continue` at module top level reached evaluation — \
                      `saule_semantic::analyze` was not run on this module"
                .to_string(),
            span: 0..0,
        }),
    }
}
