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
}
