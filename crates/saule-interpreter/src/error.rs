//! Runtime errors surfaced by the interpreter.

use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum RuntimeError {
    #[error("undefined variable `{name}`")]
    Undefined {
        name: String,
        #[label("not in scope")]
        span: std::ops::Range<usize>,
    },

    #[error("cannot assign to undeclared variable `{name}`")]
    AssignUndeclared {
        name: String,
        #[label("declare it first with `local`")]
        span: std::ops::Range<usize>,
    },

    #[error("invalid assignment target")]
    InvalidAssignTarget {
        #[label("only identifiers can be assigned in this phase")]
        span: std::ops::Range<usize>,
    },

    #[error("type error: {message}")]
    TypeError {
        message: String,
        #[label("here")]
        span: std::ops::Range<usize>,
    },

    #[error("cannot mix `integer` and `float` in arithmetic")]
    NumericMix {
        #[label("use int() or float() to convert explicitly")]
        span: std::ops::Range<usize>,
    },

    #[error("division by zero")]
    DivisionByZero {
        #[label("here")]
        span: std::ops::Range<usize>,
    },

    #[error("numeric `for` requires a non-zero step")]
    ZeroStep {
        #[label("step evaluates to zero")]
        span: std::ops::Range<usize>,
    },

    #[error("`{which}` used outside of a loop")]
    LoopControlOutsideLoop {
        which: &'static str,
        #[label("here")]
        span: std::ops::Range<usize>,
    },

    #[error("`return` used outside of a function")]
    ReturnOutsideFunction {
        #[label("here")]
        span: std::ops::Range<usize>,
    },

    #[error("`{thing}` is not yet implemented")]
    Unsupported {
        thing: &'static str,
        #[label("not handled by the interpreter")]
        span: std::ops::Range<usize>,
    },
}

/// Convenience constructor — most non-`Local`/`Expr` statements still funnel
/// through this until the matching phase lands.
pub fn unsupported(thing: &'static str, span: std::ops::Range<usize>) -> RuntimeError {
    RuntimeError::Unsupported { thing, span }
}
