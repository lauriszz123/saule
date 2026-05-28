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

pub mod env;
pub mod error;
pub mod eval;
pub mod project;
pub mod stdlib;
pub mod value;
pub mod module;

/// Re-export of the standalone `saule-typeck` crate so existing call sites
/// (`saule_interpreter::typeck::check`) keep working after the extraction.
pub use saule_typeck as typeck;

/// Re-export of the `saule-semantic` crate. The standard pipeline runs
/// `semantic::analyze` before `typeck::check`, and both are then followed
/// by [`run_in`].
pub use saule_semantic as semantic;

pub use env::Environment;
pub use error::RuntimeError;
pub use eval::Flow;
pub use value::{NativeFn, Value};

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
    eval::expr::call_static_method_public(&f, class, &evaled, 0..0)
}

/// Run a parsed [`Module`] in a fresh environment seeded with built-ins.
///
/// Returns the value of the last evaluated expression-statement (useful for
/// the REPL and for tests).
pub fn run(module: &Module) -> Result<Value, RuntimeError> {
    init();
    let env = Environment::with_prelude();
    run_in(module, &env)
}

/// Run a [`Module`] inside a caller-supplied environment.
pub fn run_in(module: &Module, env: &Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
    init();
    match eval::stmt::exec_block(&module.stmts, env)? {
        Flow::Normal(v) => Ok(v),
        // At the top level these are illegal — `Stmt::Return` is rejected
        // inside `exec`; `break`/`continue` reach here only if they appear
        // outside any loop.
        Flow::Break => Err(RuntimeError::LoopControlOutsideLoop {
            which: "break",
            span: 0..0,
        }),
        Flow::Continue => Err(RuntimeError::LoopControlOutsideLoop {
            which: "continue",
            span: 0..0,
        }),
        Flow::Return(values) => Ok(values.into_iter().next().unwrap_or(Value::Nil)),
    }
}