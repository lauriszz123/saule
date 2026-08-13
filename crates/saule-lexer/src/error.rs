use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum LexerError {
    #[error("unexpected character")]
    Unexpected(#[label("here")] std::ops::Range<usize>),
    #[error("unterminated string literal")]
    UnterminatedString(#[label("string starts here")] std::ops::Range<usize>),
    #[error("unterminated block comment")]
    UnterminatedBlockComment(#[label("comment starts here")] std::ops::Range<usize>),
    #[error("invalid escape sequence")]
    BadEscape(#[label("here")] std::ops::Range<usize>),
    #[error("invalid number literal")]
    BadNumber(#[label("here")] std::ops::Range<usize>),
}
