//! Semantic diagnostics — issues that don't depend on types but require
//! understanding the whole program (declared names, scopes, control flow,
//! field initialization). Emitted by [`crate::analyze`] before typechecking.

use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum SemanticError {
    #[error("field `{field}` of class `{class}` is never initialized")]
    #[diagnostic(help(
        "assign `self.{field} = ...` in `init`, give the field a default value, or mark it nullable with `?`"
    ))]
    FieldNotInitialized {
        class: String,
        field: String,
        #[label("declared here")]
        span: miette::SourceSpan,
    },

    #[error("`{which}` is only valid inside a loop")]
    #[diagnostic(help("move this inside a `for`, `while`, or `repeat` loop"))]
    LoopControlOutsideLoop {
        which: &'static str,
        #[label("not inside a loop")]
        span: miette::SourceSpan,
    },

    #[error("`return` is only valid inside a function")]
    #[diagnostic(help("move this statement inside a `fn` or method definition"))]
    ReturnOutsideFunction {
        #[label("not inside a function")]
        span: miette::SourceSpan,
    },

    #[error("undefined name `{name}` — it is not declared in this scope")]
    #[diagnostic(help("declare it with `local {name} = ...`, import it, or check the spelling"))]
    UndefinedName {
        name: String,
        #[label("not defined")]
        span: miette::SourceSpan,
    },

    #[error("cannot assign to undeclared variable `{name}`")]
    #[diagnostic(help("use `local {name} = ...` to declare it first"))]
    AssignToUndeclared {
        name: String,
        #[label("not declared")]
        span: miette::SourceSpan,
    },
}

