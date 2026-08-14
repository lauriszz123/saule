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
    Amp,   // `&` bitwise and
    Pipe,  // `|` bitwise or
    Tilde, // `~` bitwise xor (binary) / complement (unary)
    Shl,   // `<<`
    Shr,   // `>>`
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

    // Compound assignment. `a op= b` updates `a` in place; see
    // `Stmt::CompoundAssign` for why these are not desugared at parse time.
    PlusEq,    // `+=`
    MinusEq,   // `-=`
    StarEq,    // `*=`
    SlashEq,   // `/=`
    PercentEq, // `%=`
    CaretEq,   // `^=`
    DotDotEq,  // `..=`
    AmpEq,     // `&=`
    PipeEq,    // `|=`
    ShlEq,     // `<<=`
    ShrEq,     // `>>=`
    //
    // There is deliberately no `~=`. In Lua that spelling means "not equal",
    // which Saule writes `!=`; lexing it as xor-assignment would silently
    // turn a habitual `if a ~= b` into an assignment statement. Left
    // unlexed, `a ~= b` is a parse error, which is what a Lua reflex
    // deserves here.

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
