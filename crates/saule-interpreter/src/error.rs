//! Runtime errors surfaced by the interpreter.
//!
//! Kept disjoint from the compile-time error families:
//!
//! * `saule_lexer::LexerError` — lexing
//! * `saule_parser::ParseError` — parsing
//! * `saule_semantic::SemanticError` — structural / control-flow / definite
//!   assignment / name resolution
//! * `saule_typeck::TypeCheckError` — types, nullability, match exhaustiveness,
//!   unknown members, function arity, …
//! * `RuntimeError` (this file) — only genuinely-dynamic failures: division
//!   by zero, force-unwrap of `nil`, uncaught `throw`, file-I/O, etc.
//!
//! The variants below are the residual set after semantic + typeck have run.
//! Anything caught by an earlier pass has been removed; if you find yourself
//! about to add a new variant here, consider whether semantic or typeck is
//! the more appropriate owner first.

use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum RuntimeError {
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

    #[error("argument error: {message}")]
    ArgumentError {
        message: String,
        #[help]
        help: Option<String>,
        #[label("invalid or missing call argument")]
        span: std::ops::Range<usize>,
    },

    #[error("cannot mix `integer` and `float` in arithmetic — type mismatch")]
    NumericMix {
        #[label("cast one side with `as integer` / `as float` to make the types match")]
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

    #[error("force-unwrapped a `nil` value")]
    #[diagnostic(help(
        "use `??` to provide a fallback, or check with `if x != nil` before unwrapping"
    ))]
    ForceUnwrapNil {
        #[label("this expression was `nil` when `!` was applied")]
        span: std::ops::Range<usize>,
    },

    #[error("`{thing}` is not yet implemented")]
    Unsupported {
        thing: &'static str,
        #[label("this feature will be supported in a future version")]
        span: std::ops::Range<usize>,
    },

    #[error("import error: {message}")]
    ImportError {
        message: String,
        #[label("could not resolve this import")]
        span: std::ops::Range<usize>,
    },

    /// An error that bubbled up from an imported module. Carries its own
    /// `NamedSource` so miette renders the snippet from the *imported* file
    /// rather than the importer's source. The original error is preserved
    /// as a nested `#[diagnostic_source]` so the user sees both the import
    /// site (in the parent file) and the actual offending line.
    #[error("error in imported module `{module_label}`")]
    ImportFailed {
        module_label: String,
        #[label("imported here")]
        import_span: std::ops::Range<usize>,
        #[diagnostic_source]
        inner: Box<dyn miette::Diagnostic + Send + Sync + 'static>,
    },

    /// A runtime error fired while executing code defined in an imported
    /// module *after* it had finished loading (e.g. a method called from
    /// the entry file later on). Carries the imported module's source so
    /// miette renders the snippet from the right file even though no
    /// `import` statement is on the call stack at the moment of failure.
    #[error("runtime error in `{module_label}`")]
    InModule {
        module_label: String,
        #[diagnostic_source]
        inner: Box<dyn miette::Diagnostic + Send + Sync + 'static>,
    },

    /// A `throw` expression that hasn't been caught yet. Propagates through
    /// `Result<_, RuntimeError>` until either a matching `try ... catch`
    /// captures it, or it reaches the top of the program (where it becomes
    /// an unhandled exception). The actual thrown `Value` is parked in a
    /// thread-local slot (see `crate::eval::stmt::thrown_slot`) because
    /// `RuntimeError` must be `Send + Sync` for miette.
    #[error("uncaught exception: {value}")]
    #[diagnostic(help("wrap the throwing code in `try ... catch e: <Type> ... end` to handle it"))]
    Thrown {
        /// Display form of the thrown value.
        value: String,
        #[label("uncaught `throw`")]
        span: std::ops::Range<usize>,
    },

    /// Evaluation nested deeper than [`crate::eval::MAX_EVAL_DEPTH`].
    ///
    /// The interpreter is a recursive tree-walker, so a deeply-recursive
    /// Saule program consumes the *native* stack. Without this guard the
    /// process dies with `fatal runtime error: stack overflow` — no span,
    /// no message, no chance for a `catch` to run, and in the language
    /// server it takes the whole editor session down. The counter turns
    /// that into an ordinary catchable error.
    #[error("stack overflow: evaluation nested more than {limit} levels deep")]
    #[diagnostic(help(
        "this is usually unbounded recursion — check that the recursive call has a base case"
    ))]
    StackOverflow {
        limit: u32,
        #[label("while evaluating this")]
        span: std::ops::Range<usize>,
    },

    /// Sentinel: a `return` / `break` / `continue` statement executed
    /// inside an expression context (e.g. a `match` arm). The actual
    /// `Flow` is parked in `crate::eval::stmt::pending_flow` and the
    /// surrounding statement executor restores it. Never reaches the
    /// user; if it does, that's an interpreter bug.
    #[error("internal: control-flow escape (this should be intercepted)")]
    PendingFlow {
        #[label("control-flow escape")]
        span: std::ops::Range<usize>,
    },
}

/// Convenience constructor — most non-`Local`/`Expr` statements still funnel
/// through this until the matching phase lands.
pub fn unsupported(thing: &'static str, span: std::ops::Range<usize>) -> RuntimeError {
    RuntimeError::Unsupported { thing, span }
}

/// Diagnostic wrapper that carries an imported module's `NamedSource` so
/// miette renders the offending snippet from *that* file. Used as the
/// `#[diagnostic_source]` of [`RuntimeError::ImportFailed`].
#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
pub struct ImportedDiagnostic {
    /// The inner error's `Display` text (e.g. the parser/typecheck/runtime
    /// message). Shown as the primary error line.
    pub message: String,
    /// Source file the error happened in. Drives the snippet miette
    /// underlines.
    #[source_code]
    pub src: miette::NamedSource<String>,
    /// Primary offending span inside `src`.
    #[label("{label}")]
    pub span: miette::SourceSpan,
    /// Label text rendered under the span (often "here").
    pub label: String,
}

impl ImportedDiagnostic {
    /// Build an `ImportedDiagnostic` from any `miette::Diagnostic` plus the
    /// source it came from. Pulls the first label off the inner diagnostic
    /// if any, otherwise falls back to a zero-length span at the start.
    pub fn from_inner(
        inner: &dyn miette::Diagnostic,
        file_label: String,
        source_text: String,
    ) -> Self {
        let (span, label) = inner
            .labels()
            .and_then(|mut it| it.next())
            .map(|l| {
                let s: miette::SourceSpan = (l.offset(), l.len()).into();
                (s, l.label().unwrap_or("here").to_string())
            })
            .unwrap_or_else(|| ((0usize, 0usize).into(), "here".to_string()));

        // Compose the message: prefer the inner's `Display`; append the
        // help text if any (so e.g. typecheck hints survive the relay).
        let mut message = inner.to_string();
        if let Some(help) = inner.help() {
            let help_str = help.to_string();
            if !help_str.is_empty() {
                message.push_str("\n  help: ");
                message.push_str(&help_str);
            }
        }

        Self {
            message,
            src: miette::NamedSource::new(file_label, source_text),
            span,
            label,
        }
    }
}
