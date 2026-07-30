//! The [`Token`] enum produced by the lexer.

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords
    Class,
    Interface,
    Enum,
    Fn,
    Extends,
    Implements,
    Super,
    Self_,
    Static,
    Local,
    Export,
    Import,
    Return,
    Throw,
    Try,
    Catch,
    For,
    While,
    Repeat,
    Until,
    Break,
    Continue,
    If,
    Else,
    Elseif,
    End,
    Then,
    Do,
    In,
    As,
    From,
    Match,
    Case,
    When,
    And,
    Or,
    Not,
    Nil,
    True,
    False,

    // Identifiers and literals
    Identifier(String),
    Int(i64),
    Float(f64),
    String(String),

    // Operators / punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret, // `^` exponentiation
    EqEq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    Dot,
    DotDot,   // `..` string concatenation
    Ellipsis, // `...` variadic
    Comma,
    Colon,
    Semi,
    Assign,
    Question,
    QuestionDot,
    QuestionQuestion,
    Bang,
    Hash,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Arrow,    // `->`
    FatArrow, // `=>`

    // Trivia
    //
    // Comments are produced by [`crate::Lexer::tokenize_with_trivia`] only;
    // the default [`crate::Lexer::tokenize`] filters them out so the parser
    // and downstream pipeline never see them. `text` is the verbatim
    // payload between the comment delimiters (no `--` / `--[[`/`]]`).
    LineComment(String),
    BlockComment(String),

    // End of input
    Eof,
}
