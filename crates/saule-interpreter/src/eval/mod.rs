//! Statement & expression evaluation.
//!
//! Organized into:
//!   * [`expr`] — pure expression evaluation
//!   * [`stmt`] — statement execution with control-flow propagation
//!   * [`ops`]  — operator helpers (arithmetic / comparison / equality)
//!
//! The [`Flow`] enum is how `break` / `continue` / `return` propagate up
//! through nested blocks without using panics or special error variants.

use crate::value::Value;

pub mod expr;
pub mod ops;
pub mod stmt;

/// Result of executing a single statement (or a block of statements).
///
/// `Normal(v)` means execution should continue; `v` is the value of the
/// last expression-statement (used at the REPL and in tests).
#[derive(Debug, Clone)]
pub enum Flow {
    Normal(Value),
    Break,
    Continue,
    Return(Value),
}

impl Flow {
    /// Convenience for the common "nothing interesting happened" case.
    pub fn nil() -> Self {
        Flow::Normal(Value::Nil)
    }

    /// Returns the inner value if this is a `Normal` outcome, else `Nil`.
    pub fn into_value(self) -> Value {
        match self {
            Flow::Normal(v) | Flow::Return(v) => v,
            _ => Value::Nil,
        }
    }
}
