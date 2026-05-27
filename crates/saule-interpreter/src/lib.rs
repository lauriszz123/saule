//! Tree-walking interpreter for the Saule language.
//!
//! Module layout:
//!
//! | Module       | Responsibility                              |
//! |--------------|---------------------------------------------|
//! | [`value`]    | Runtime [`Value`] enum and `NativeFn`       |
//! | [`env`]      | Lexical scopes ([`Environment`])            |
//! | [`stdlib`]   | Standard library installed into the prelude |
//! | [`error`]    | [`RuntimeError`] (miette-aware diagnostics) |
//! | [`eval`]     | Statement & expression evaluation           |
//!
//! Phase status:
//!   * Phase 1 — literals, locals, arithmetic, native calls (✓)
//!   * Phase 2 — assignment, blocks, `if`/`while`/`repeat`/numeric `for`,
//!     `break`/`continue`, lexical scoping (✓)
//!   * Phase 3 — user-defined functions, lambdas, closures, `return` (✓ —
//!     this commit)
//!   * Phase 4 — tables and indexing (next)

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::Module;

pub mod env;
pub mod error;
pub mod eval;
pub mod project;
pub mod stdlib;
pub mod typeck;
pub mod value;
pub mod module;

pub use env::Environment;
pub use error::RuntimeError;
pub use eval::Flow;
pub use value::{NativeFn, Value};

/// Invoke a [`value::FunctionObject`] with the given arguments. Exposed so
/// embedders (the CLI, the REPL, future test runners) can call functions
/// that were defined in user code without re-parsing.
pub fn call_function_value(
    f: &std::rc::Rc<value::FunctionObject>,
    args: &[Value],
) -> Result<Value, RuntimeError> {
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
    let env = Environment::with_prelude();
    run_in(module, &env)
}

/// Run a [`Module`] inside a caller-supplied environment.
pub fn run_in(module: &Module, env: &Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
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