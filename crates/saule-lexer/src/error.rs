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
    #[error("invalid `\\x` escape")]
    #[diagnostic(help("`\\x` takes exactly two hex digits, as in `\\x41` for `A`"))]
    BadHexEscape(#[label("here")] std::ops::Range<usize>),
    #[error("invalid `\\u` escape")]
    #[diagnostic(help(
        "`\\u` takes one to six hex digits in braces, as in `\\u{{1F600}}` — and the value has to be a real character, so a surrogate (`D800`-`DFFF`) or anything past `10FFFF` is rejected"
    ))]
    BadUnicodeEscape(#[label("here")] std::ops::Range<usize>),
    #[error("invalid number literal")]
    BadNumber(#[label("here")] std::ops::Range<usize>),
}
