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

impl Token {
    /// The source text this token always stands for, or `None` for the
    /// variants whose text varies (identifiers, literals, comments) and for
    /// [`Token::Eof`], which stands for no text at all.
    ///
    /// Every caller so far wants this for a message rather than for
    /// re-emitting source, so the pointer-sized `&'static str` is deliberate:
    /// a token that carries text is exactly the case where the caller has to
    /// think about which text it means.
    pub fn lexeme(&self) -> Option<&'static str> {
        Some(match self {
            Token::Class => "class",
            Token::Interface => "interface",
            Token::Enum => "enum",
            Token::Fn => "fn",
            Token::Extends => "extends",
            Token::Implements => "implements",
            Token::Super => "super",
            Token::Self_ => "self",
            Token::Static => "static",
            Token::Local => "local",
            Token::Export => "export",
            Token::Import => "import",
            Token::Return => "return",
            Token::Throw => "throw",
            Token::Try => "try",
            Token::Catch => "catch",
            Token::For => "for",
            Token::While => "while",
            Token::Repeat => "repeat",
            Token::Until => "until",
            Token::Break => "break",
            Token::Continue => "continue",
            Token::If => "if",
            Token::Else => "else",
            Token::Elseif => "elseif",
            Token::End => "end",
            Token::Then => "then",
            Token::Do => "do",
            Token::In => "in",
            Token::As => "as",
            Token::From => "from",
            Token::Match => "match",
            Token::Case => "case",
            Token::When => "when",
            Token::And => "and",
            Token::Or => "or",
            Token::Not => "not",
            Token::Nil => "nil",
            Token::True => "true",
            Token::False => "false",

            Token::Plus => "+",
            Token::Minus => "-",
            Token::Star => "*",
            Token::Slash => "/",
            Token::Percent => "%",
            Token::Caret => "^",
            Token::Amp => "&",
            Token::Pipe => "|",
            Token::Tilde => "~",
            Token::Shl => "<<",
            Token::Shr => ">>",
            Token::EqEq => "==",
            Token::NotEq => "!=",
            Token::Lt => "<",
            Token::Gt => ">",
            Token::LtEq => "<=",
            Token::GtEq => ">=",
            Token::Dot => ".",
            Token::DotDot => "..",
            Token::Ellipsis => "...",
            Token::Comma => ",",
            Token::Colon => ":",
            Token::Semi => ";",
            Token::Assign => "=",
            Token::Question => "?",
            Token::QuestionDot => "?.",
            Token::QuestionQuestion => "??",
            Token::Bang => "!",
            Token::Hash => "#",
            Token::LParen => "(",
            Token::RParen => ")",
            Token::LBrace => "{",
            Token::RBrace => "}",
            Token::LBracket => "[",
            Token::RBracket => "]",
            Token::Arrow => "->",
            Token::FatArrow => "=>",

            Token::PlusEq => "+=",
            Token::MinusEq => "-=",
            Token::StarEq => "*=",
            Token::SlashEq => "/=",
            Token::PercentEq => "%=",
            Token::CaretEq => "^=",
            Token::DotDotEq => "..=",
            Token::AmpEq => "&=",
            Token::PipeEq => "|=",
            Token::ShlEq => "<<=",
            Token::ShrEq => ">>=",

            Token::Identifier(_)
            | Token::Int(_)
            | Token::Float(_)
            | Token::String(_)
            | Token::LineComment(_)
            | Token::BlockComment(_)
            | Token::Eof => return None,
        })
    }

    /// Whether this token is a reserved word.
    ///
    /// Only used to *say so* in a diagnostic. "found `end`" leaves the reader
    /// to work out why their variable name was rejected; "found keyword
    /// `end`" has already answered it.
    pub fn is_keyword(&self) -> bool {
        matches!(
            self,
            Token::Class
                | Token::Interface
                | Token::Enum
                | Token::Fn
                | Token::Extends
                | Token::Implements
                | Token::Super
                | Token::Self_
                | Token::Static
                | Token::Local
                | Token::Export
                | Token::Import
                | Token::Return
                | Token::Throw
                | Token::Try
                | Token::Catch
                | Token::For
                | Token::While
                | Token::Repeat
                | Token::Until
                | Token::Break
                | Token::Continue
                | Token::If
                | Token::Else
                | Token::Elseif
                | Token::End
                | Token::Then
                | Token::Do
                | Token::In
                | Token::As
                | Token::From
                | Token::Match
                | Token::Case
                | Token::When
                | Token::And
                | Token::Or
                | Token::Not
                | Token::Nil
                | Token::True
                | Token::False
        )
    }

    /// How this token is named inside a diagnostic, as the `found …` half of
    /// an "expected X, found Y" message.
    ///
    /// The half that carries the information is the *category*: a parser can
    /// only ever point at one token, and the caret in the rendered snippet
    /// already shows which one it is. What the caret cannot show is that the
    /// thing under it is a reserved word, or the end of the file, which is
    /// almost always the reason the parse stopped there.
    ///
    /// String literals and comments are named by category rather than
    /// quoted: their text is unbounded, and a message that inlines a 400-byte
    /// string is worse than one that says which kind of token it found.
    pub fn describe(&self) -> std::borrow::Cow<'static, str> {
        match self {
            Token::Identifier(name) => format!("identifier `{name}`").into(),
            Token::Int(n) => format!("number `{n}`").into(),
            Token::Float(f) => format!("number `{f}`").into(),
            Token::String(_) => "a string literal".into(),
            Token::LineComment(_) | Token::BlockComment(_) => "a comment".into(),
            Token::Eof => "end of input".into(),
            fixed => {
                let text = fixed
                    .lexeme()
                    .expect("every variant without a fixed lexeme is handled above");
                if fixed.is_keyword() {
                    format!("keyword `{text}`").into()
                } else {
                    format!("`{text}`").into()
                }
            }
        }
    }
}
