//! Runtime errors surfaced by the interpreter.

use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum RuntimeError {
    #[error("undefined variable `{name}` — it is not in scope")]
    Undefined {
        name: String,
        #[label("not defined; declare it with `local` or check the spelling")]
        span: std::ops::Range<usize>,
    },

    #[error("cannot assign to undeclared variable `{name}` — use `local {name} = ...` to declare it first")]
    AssignUndeclared {
        name: String,
        #[label("declare with `local` before assigning")]
        span: std::ops::Range<usize>,
    },

    #[error("invalid assignment target")]
    InvalidAssignTarget {
        #[label("only identifiers and member/index access can be assigned to")]
        span: std::ops::Range<usize>,
    },

    #[error("type error: {message}")]
    TypeError {
        message: String,
        #[label("type mismatch or invalid operation")]
        span: std::ops::Range<usize>,
    },

    #[error("cannot mix `integer` and `float` in arithmetic — type mismatch")]
    NumericMix {
        #[label("convert using int() or float() to make types compatible")]
        span: std::ops::Range<usize>,
    },

    #[error("division by zero — cannot divide by 0")]
    DivisionByZero {
        #[label("ensure the divisor is not zero")]
        span: std::ops::Range<usize>,
    },

    #[error("numeric `for` loop requires a non-zero step value")]
    ZeroStep {
        #[label("step cannot be 0 — use a positive or negative number")]
        span: std::ops::Range<usize>,
    },

    #[error("`{which}` is only valid inside a loop")]
    LoopControlOutsideLoop {
        which: &'static str,
        #[label("move this inside a `for`, `while`, or `repeat` loop")]
        span: std::ops::Range<usize>,
    },

    #[error("`return` is only valid inside a function")]
    ReturnOutsideFunction {
        #[label("move this statement inside a `fn` definition")]
        span: std::ops::Range<usize>,
    },

    #[error("`{thing}` is not yet implemented")]
    Unsupported {
        thing: &'static str,
        #[label("this feature will be supported in a future version")]
        span: std::ops::Range<usize>,
    },
}

/// Convenience constructor — most non-`Local`/`Expr` statements still funnel
/// through this until the matching phase lands.
pub fn unsupported(thing: &'static str, span: std::ops::Range<usize>) -> RuntimeError {
    RuntimeError::Unsupported { thing, span }
}
