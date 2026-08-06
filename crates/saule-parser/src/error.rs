use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum ParseError {
    #[error("unexpected token")]
    Unexpected {
        #[label("here")]
        span: std::ops::Range<usize>,
    },
    #[error("expected {expected}")]
    Expected {
        expected: &'static str,
        #[label("here")]
        span: std::ops::Range<usize>,
    },
    #[error("unexpected end of input")]
    Eof {
        #[label("end of file")]
        span: std::ops::Range<usize>,
    },
    /// The grammar nested deeper than [`crate::MAX_NESTING_DEPTH`].
    ///
    /// This is a recursive-descent parser, so nesting in the source becomes
    /// recursion on the native stack. Without a bound, input like 50k stacked
    /// `(` aborts the process instead of reporting an error — and the language
    /// server parses half-typed input constantly, where an abort takes the
    /// editor session with it.
    #[error("expression nests more than {limit} levels deep")]
    #[diagnostic(help(
        "simplify the expression, or split it across intermediate `local` bindings"
    ))]
    TooDeep {
        limit: u32,
        #[label("nesting starts here")]
        span: std::ops::Range<usize>,
    },
}
