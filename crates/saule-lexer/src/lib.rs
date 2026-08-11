//! Lexer for Saule.
//!
//! ## Error recovery
//!
//! Lexing never stops at the first problem. Each of the five ways a character
//! run can be wrong has a repair that keeps the token stream shaped like the
//! code the author is writing:
//!
//! | Problem | Repair |
//! |---------|--------|
//! | Unexpected character | dropped; no token emitted |
//! | Unterminated string  | closed at the end of its line — see [`Lexer::string`] |
//! | Bad escape (`"\q"`)  | the character is kept literally |
//! | Unterminated block comment | closed at end of file |
//! | Malformed number     | becomes `0`, keeping its span |
//!
//! [`Lexer::tokenize`] still reports the first error and no tokens, for the
//! callers that must not act on a guess. [`Lexer::tokenize_recover`] returns
//! both the tokens and every error, for the language server — the two agree on
//! *which* input is wrong, and differ only in whether a token stream comes
//! back with the diagnostic.

mod error;
mod token;

pub use error::LexerError;
use saule_ast::Spanned;
pub use token::Token;

/// How many lexical errors are worth reporting from one file. Matches the
/// parser's cap for the same reason: past a certain point nobody reads them.
pub const MAX_ERRORS: usize = 64;

/// A recovered lex: the tokens, plus everything that went wrong producing
/// them. `errors` is empty exactly when the input was clean.
#[derive(Debug)]
pub struct Lexed {
    pub tokens: Vec<Spanned<Token>>,
    pub errors: Vec<LexerError>,
}

impl Lexed {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    fn without_trivia(mut self) -> Self {
        self.tokens
            .retain(|t| !matches!(t.value, Token::LineComment(_) | Token::BlockComment(_)));
        self
    }

    fn into_result(self) -> Result<Vec<Spanned<Token>>, LexerError> {
        match self.errors.into_iter().next() {
            // Recovery begins only after an error is recorded, so this is the
            // same error, at the same span, that bailing would have produced.
            Some(first) => Err(first),
            None => Ok(self.tokens),
        }
    }
}

pub struct Lexer<'src> {
    source: &'src str,
    chars: std::iter::Peekable<std::str::CharIndices<'src>>,
    errors: Vec<LexerError>,
}

impl<'src> Lexer<'src> {
    pub fn new(source: &'src str) -> Self {
        Self {
            source,
            chars: source.char_indices().peekable(),
            errors: Vec::new(),
        }
    }

    /// Tokenize and discard comments. The default entry point: matches
    /// the lexer's historical behaviour and keeps the parser unaware of
    /// trivia.
    pub fn tokenize(self) -> Result<Vec<Spanned<Token>>, LexerError> {
        self.run().without_trivia().into_result()
    }

    /// Tokenize and preserve `--` line / `--[[ … ]]` block comments as
    /// `Token::LineComment` / `Token::BlockComment` with their full span
    /// (including delimiters). Used by the formatter to round-trip
    /// comments; the parser still goes through [`Self::tokenize`].
    pub fn tokenize_with_trivia(self) -> Result<Vec<Spanned<Token>>, LexerError> {
        self.run().into_result()
    }

    /// [`Self::tokenize`], recovering: always a token stream, plus every
    /// error found producing it.
    pub fn tokenize_recover(self) -> Lexed {
        self.run().without_trivia()
    }

    /// [`Self::tokenize_with_trivia`], recovering.
    pub fn tokenize_with_trivia_recover(self) -> Lexed {
        self.run()
    }

    /// Record a lexical error. Capped at [`MAX_ERRORS`]; lexing itself never
    /// stops, since the token stream is the reason recovery exists.
    fn error(&mut self, e: LexerError) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(e);
        }
    }

    fn run(mut self) -> Lexed {
        let mut out = Vec::new();

        while let Some(&(start, c)) = self.chars.peek() {
            // Whitespace
            if c.is_whitespace() {
                self.chars.next();
                continue;
            }

            // Lua-style comments: `--` line, `--[[ ... ]]` block.
            if c == '-' {
                let mut look = self.chars.clone();
                look.next(); // skip first '-'

                if matches!(look.peek(), Some(&(_, '-'))) {
                    // Consume the two '-'s.
                    self.chars.next();
                    self.chars.next();

                    // Block comment opener `--[[`?
                    let mut look2 = self.chars.clone();
                    let is_block = matches!(look2.next(), Some((_, '[')))
                        && matches!(look2.peek(), Some(&(_, '[')));

                    if is_block {
                        self.chars.next(); // consume '['
                        self.chars.next(); // consume '['
                        let body_start = start + 4; // after `--[[`
                        // `Some(i)` — the first `]` of the closing pair.
                        // `None` — no `]]` anywhere, so the comment is
                        // unterminated; a block comment is meant to span
                        // lines, and there is no better guess than "to the
                        // end of the file".
                        let mut close = None;
                        loop {
                            match self.chars.next() {
                                Some((i, ']')) if matches!(self.chars.peek(), Some(&(_, ']'))) => {
                                    self.chars.next();
                                    close = Some(i);
                                    break;
                                }
                                Some(_) => {}
                                None => {
                                    self.error(LexerError::UnterminatedBlockComment(
                                        start..self.source.len(),
                                    ));
                                    break;
                                }
                            }
                        }
                        let (body_end, span_end) = match close {
                            Some(i) => (i, i + 2), // the span includes the `]]`
                            None => (self.source.len(), self.source.len()),
                        };
                        let text = self.source[body_start.min(body_end)..body_end].to_string();
                        out.push(Spanned {
                            value: Token::BlockComment(text),
                            span: start..span_end,
                        });
                    } else {
                        // Line comment: until end of line (or EOF).
                        let body_start = start + 2; // after `--`
                        let mut end = body_start;
                        while let Some(&(i, ch)) = self.chars.peek() {
                            if ch == '\n' {
                                break;
                            }
                            self.chars.next();
                            end = i + ch.len_utf8();
                        }
                        let text = self.source[body_start..end].to_string();
                        out.push(Spanned {
                            value: Token::LineComment(text),
                            span: start..end,
                        });
                    }
                    continue;
                }
                // Not a comment — fall through so '-' becomes Minus or Arrow.
            }

            // A `.` immediately followed by a digit opens a leading-dot float
            // (`.5`). Checking the *next* character is what keeps `..` and
            // `...` intact: in `1..2` the char after the first dot is another
            // dot, so this never fires and `..` still lexes as concat.
            let dot_starts_float = c == '.' && {
                let mut look = self.chars.clone();
                look.next();
                matches!(look.peek(), Some(&(_, d)) if d.is_ascii_digit())
            };

            let tok = match c {
                '0'..='9' => Some(self.number(start)),
                _ if dot_starts_float => Some(self.number(start)),
                'a'..='z' | 'A'..='Z' | '_' => Some(self.ident_or_keyword(start)),
                '"' | '\'' => Some(self.string(start, c)),
                // The only repair that emits nothing: an unexpected character
                // stands for no token at all, so dropping it is exactly right
                // and leaves the tokens on either side adjacent.
                _ => self.symbol(start),
            };

            out.extend(tok);
        }

        let end = self.source.len();
        out.push(Spanned {
            value: Token::Eof,
            span: end..end,
        });

        Lexed {
            tokens: out,
            errors: self.errors,
        }
    }

    fn ident_or_keyword(&mut self, start: usize) -> Spanned<Token> {
        let mut end = start;
        while let Some(&(i, c)) = self.chars.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                end = i + c.len_utf8();
                self.chars.next();
            } else {
                break;
            }
        }
        let text = &self.source[start..end];
        let tok = match text {
            "class" => Token::Class,
            "fn" => Token::Fn,
            "if" => Token::If,
            "else" => Token::Else,
            "elseif" => Token::Elseif,
            "true" => Token::True,
            "false" => Token::False,
            "nil" => Token::Nil,
            "return" => Token::Return,
            "import" => Token::Import,
            "export" => Token::Export,
            "extends" => Token::Extends,
            "implements" => Token::Implements,
            "interface" => Token::Interface,
            "enum" => Token::Enum,
            "super" => Token::Super,
            "self" => Token::Self_,
            "static" => Token::Static,
            "local" => Token::Local,
            "throw" => Token::Throw,
            "try" => Token::Try,
            "catch" => Token::Catch,
            "for" => Token::For,
            "while" => Token::While,
            "repeat" => Token::Repeat,
            "until" => Token::Until,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "end" => Token::End,
            "then" => Token::Then,
            "do" => Token::Do,
            "in" => Token::In,
            "as" => Token::As,
            "from" => Token::From,
            "match" => Token::Match,
            "case" => Token::Case,
            "when" => Token::When,
            "and" => Token::And,
            "or" => Token::Or,
            "not" => Token::Not,
            _ => Token::Identifier(text.to_string()),
        };
        Spanned {
            value: tok,
            span: start..end,
        }
    }

    /// Lex a numeric literal.
    ///
    /// Accepts `12`, `1.5`, `.5` (the integer part may be omitted), a trailing
    /// `f`/`F` suffix that forces a float — so `1f` is `1.0` — and the
    /// base-prefixed integer forms `0xFF` and `0b1010`.
    fn number(&mut self, start: usize) -> Spanned<Token> {
        let mut end = start;
        let mut is_float = false;

        // A base prefix has to be checked before the decimal scan, or the `0`
        // would be eaten as an ordinary integer and `x` left as an identifier.
        // Nothing is consumed yet at this point, so the marker is the character
        // *after* the leading zero.
        if self.source[start..].starts_with('0') {
            let mut look = self.chars.clone();
            look.next();

            if let Some(&(_, marker)) = look.peek() {
                let radix = match marker {
                    'x' | 'X' => Some(16),
                    'b' | 'B' => Some(2),
                    _ => None,
                };

                if let Some(radix) = radix {
                    self.chars.next(); // the leading `0`

                    return self.radix_number(start, radix);
                }
            }
        }

        // Integer part. Consumes nothing for a leading-dot literal like `.5`,
        // which the fractional branch below picks up.
        while let Some(&(i, c)) = self.chars.peek() {
            if c.is_ascii_digit() {
                end = i + c.len_utf8();
                self.chars.next();
            } else {
                break;
            }
        }

        // Optional fractional part. Only consume `.` if a digit follows, so
        // `1..2` (concat) and `1.foo` (member access) still tokenize correctly.
        if let Some(&(dot_i, '.')) = self.chars.peek() {
            let mut look = self.chars.clone();
            look.next(); // skip '.'
            if matches!(look.peek(), Some(&(_, d)) if d.is_ascii_digit()) {
                is_float = true;
                self.chars.next(); // consume '.'
                end = dot_i + 1;
                while let Some(&(i, c)) = self.chars.peek() {
                    if c.is_ascii_digit() {
                        end = i + c.len_utf8();
                        self.chars.next();
                    } else {
                        break;
                    }
                }
            }
        }

        // The numeric text ends here; a suffix extends the span but must stay
        // out of the string handed to `parse`.
        let digits_end = end;

        // Optional `f` / `F` suffix, so an integral value can be written as a
        // float without a fractional part: `1f` is `1.0`. Only treat it as a
        // suffix when no further identifier character follows, which keeps
        // `1foo` lexing as `1` then `foo` rather than `1f` then `oo`.
        if let Some(&(sfx_i, sfx)) = self.chars.peek()
            && (sfx == 'f' || sfx == 'F')
        {
            let mut look = self.chars.clone();
            look.next(); // skip the suffix
            let continues_ident =
                matches!(look.peek(), Some(&(_, c)) if c.is_alphanumeric() || c == '_');
            if !continues_ident {
                self.chars.next();
                is_float = true;
                end = sfx_i + sfx.len_utf8();
            }
        }

        let text = &self.source[start..digits_end];
        // A literal that doesn't parse still occupies a value position, so the
        // repair is a number — `0`, keeping the original span. Dropping the
        // token instead would leave a hole in an expression and turn one bad
        // literal into a cascade of parse errors around it.
        let tok = if is_float {
            match text.parse() {
                Ok(f) => Token::Float(f),
                Err(_) => {
                    self.error(LexerError::BadNumber(start..end));
                    Token::Float(0.0)
                }
            }
        } else {
            match text.parse() {
                Ok(n) => Token::Int(n),
                Err(_) => {
                    self.error(LexerError::BadNumber(start..end));
                    Token::Int(0)
                }
            }
        };

        Spanned {
            value: tok,
            span: start..end,
        }
    }

    /// Lex `0x…` / `0b…` after the leading `0`, with `radix` already chosen.
    ///
    /// Underscores are allowed as digit separators (`0xFF_FF`), because the
    /// literals this form is for — colour values, bit masks, codepoints — are
    /// exactly the ones that are hard to read in one run.
    fn radix_number(&mut self, start: usize, radix: u32) -> Spanned<Token> {
        // Consume the base marker itself.
        let (marker_i, marker) = self.chars.next().expect("peeked by the caller");
        let mut end = marker_i + marker.len_utf8();
        let mut digits = String::new();

        while let Some(&(i, c)) = self.chars.peek() {
            if c == '_' {
                end = i + c.len_utf8();
                self.chars.next();
            } else if c.is_digit(radix) {
                digits.push(c);
                end = i + c.len_utf8();
                self.chars.next();
            } else if c.is_alphanumeric() {
                // `0xGG` or `0b2`: consume the offending run so the error span
                // covers the whole literal rather than stopping mid-token.
                end = i + c.len_utf8();
                self.chars.next();
                digits.push(c);
            } else {
                break;
            }
        }

        // `0x` with nothing after it is not a number. As in `number`, the
        // repair is `0` over the literal's own span.
        let value = if digits.is_empty() {
            self.error(LexerError::BadNumber(start..end));
            0
        } else {
            match i64::from_str_radix(&digits, radix) {
                Ok(v) => v,
                Err(_) => {
                    self.error(LexerError::BadNumber(start..end));
                    0
                }
            }
        };

        Spanned {
            value: Token::Int(value),
            span: start..end,
        }
    }

    fn symbol(&mut self, start: usize) -> Option<Spanned<Token>> {
        let (_, c) = self.chars.next().unwrap();
        let (tok, len) = match c {
            '+' => match self.chars.peek() {
                Some(&(_, '=')) => {
                    self.chars.next();
                    (Token::PlusEq, 2)
                }
                _ => (Token::Plus, 1),
            },
            // `--` never reaches here: the comment scan in
            // `tokenize_with_trivia` claims it before `symbol` is called.
            '-' => match self.chars.peek() {
                Some(&(_, '>')) => {
                    self.chars.next();
                    (Token::Arrow, 2)
                }
                Some(&(_, '=')) => {
                    self.chars.next();
                    (Token::MinusEq, 2)
                }
                _ => (Token::Minus, 1),
            },
            '=' => match self.chars.peek() {
                Some(&(_, '=')) => {
                    self.chars.next();
                    (Token::EqEq, 2)
                }
                Some(&(_, '>')) => {
                    self.chars.next();
                    (Token::FatArrow, 2)
                }
                _ => (Token::Assign, 1),
            },
            '!' => match self.chars.peek() {
                Some(&(_, '=')) => {
                    self.chars.next();
                    (Token::NotEq, 2)
                }
                _ => (Token::Bang, 1),
            },
            '<' => match self.chars.peek() {
                Some(&(_, '=')) => {
                    self.chars.next();
                    (Token::LtEq, 2)
                }
                _ => (Token::Lt, 1),
            },
            '>' => match self.chars.peek() {
                Some(&(_, '=')) => {
                    self.chars.next();
                    (Token::GtEq, 2)
                }
                _ => (Token::Gt, 1),
            },
            '?' => match self.chars.peek() {
                Some(&(_, '.')) => {
                    self.chars.next();
                    (Token::QuestionDot, 2)
                }
                Some(&(_, '?')) => {
                    self.chars.next();
                    (Token::QuestionQuestion, 2)
                }
                _ => (Token::Question, 1),
            },
            '.' => match self.chars.peek() {
                Some(&(_, '.')) => {
                    self.chars.next();
                    // Third '.' to form `...` (variadic)?
                    if matches!(self.chars.peek(), Some(&(_, '.'))) {
                        self.chars.next();
                        (Token::Ellipsis, 3)
                    } else if matches!(self.chars.peek(), Some(&(_, '='))) {
                        self.chars.next();
                        (Token::DotDotEq, 3)
                    } else {
                        (Token::DotDot, 2)
                    }
                }
                _ => (Token::Dot, 1),
            },
            '(' => (Token::LParen, 1),
            ')' => (Token::RParen, 1),
            '{' => (Token::LBrace, 1),
            '}' => (Token::RBrace, 1),
            '[' => (Token::LBracket, 1),
            ']' => (Token::RBracket, 1),
            ',' => (Token::Comma, 1),
            ':' => (Token::Colon, 1),
            ';' => (Token::Semi, 1),
            '*' => match self.chars.peek() {
                Some(&(_, '=')) => {
                    self.chars.next();
                    (Token::StarEq, 2)
                }
                _ => (Token::Star, 1),
            },
            '/' => match self.chars.peek() {
                Some(&(_, '=')) => {
                    self.chars.next();
                    (Token::SlashEq, 2)
                }
                _ => (Token::Slash, 1),
            },
            '%' => match self.chars.peek() {
                Some(&(_, '=')) => {
                    self.chars.next();
                    (Token::PercentEq, 2)
                }
                _ => (Token::Percent, 1),
            },
            '^' => match self.chars.peek() {
                Some(&(_, '=')) => {
                    self.chars.next();
                    (Token::CaretEq, 2)
                }
                _ => (Token::Caret, 1),
            },
            '#' => (Token::Hash, 1),
            _ => {
                // The character was already consumed above, so dropping it is
                // all the repair there is — and the right one: a character
                // that spells no token leaves the tokens around it adjacent,
                // exactly as if it had never been typed.
                self.error(LexerError::Unexpected(start..start + c.len_utf8()));
                return None;
            }
        };
        Some(Spanned {
            value: tok,
            span: start..start + len,
        })
    }

    /// Lex a string literal delimited by `quote` — `"` or `'`.
    ///
    /// The two spellings are interchangeable, as in Lua: only the delimiter
    /// that opened the literal closes it, so the *other* quote needs no
    /// escaping inside it (`'he said "hi"'`). Both `\"` and `\'` are accepted
    /// in either kind, so moving a literal between them never invalidates an
    /// escape that was already there.
    ///
    /// The delimiter is not recorded on the token: `Token::String` carries the
    /// decoded value, and nothing downstream can tell the two spellings apart.
    /// See `quote_str` in `saule-fmt` for how the formatter picks one again.
    /// ## Recovery
    ///
    /// An unterminated literal is closed **at the end of its own line**, not
    /// at the end of the file. This is the whole reason the closing quote is
    /// located up front rather than discovered by the scan below: the moment
    /// someone types `local s = "hello`, every remaining line of the file is
    /// syntactically inside that literal, and a repair that ran to EOF would
    /// delete the rest of the program from the token stream — precisely when
    /// the editor is being asked about it.
    ///
    /// A literal *may* legally span lines, so the line limit is applied only
    /// once we know there is no closing quote anywhere. That keeps a valid
    /// multi-line string lexing exactly as before.
    ///
    /// A bad escape keeps its character verbatim: `"\q"` lexes as `\q`, which
    /// is what the author typed and what they are about to fix.
    fn string(&mut self, start: usize, quote: char) -> Spanned<Token> {
        self.chars.next(); // consume the opening quote
        let limit = match self.closing_quote(start, quote) {
            Some(_) => self.source.len(),
            None => self.line_end(start),
        };

        let mut s = String::new();
        let mut end = None;
        while end.is_none() {
            // Stopping *before* consuming keeps the newline itself unconsumed,
            // so the next line lexes normally.
            if matches!(self.chars.peek(), Some(&(i, _)) if i >= limit) {
                break;
            }
            match self.chars.next() {
                Some((i, c)) if c == quote => end = Some(i + c.len_utf8()),
                // A `\` as the last character on the line of an unterminated
                // literal (`"abc\`, still being typed): the newline is not an
                // escapee, it is the limit. Consuming it would step past the
                // line the repair is trying to stop at, and report a bad
                // escape for the line break on the way.
                Some((_, '\\')) if matches!(self.chars.peek(), Some(&(i, _)) if i >= limit) => {
                    s.push('\\');
                }
                Some((_, '\\')) => match self.chars.next() {
                    Some((_, 'n')) => s.push('\n'),
                    Some((_, 't')) => s.push('\t'),
                    Some((_, 'r')) => s.push('\r'),
                    Some((_, '0')) => s.push('\0'),
                    Some((_, '\\')) => s.push('\\'),
                    Some((_, '"')) => s.push('"'),
                    Some((_, '\'')) => s.push('\''),
                    Some((i, c)) => {
                        self.error(LexerError::BadEscape(i..i + c.len_utf8()));
                        s.push('\\');
                        s.push(c);
                    }
                    None => break,
                },
                Some((_, c)) => s.push(c),
                None => break,
            }
        }

        let end = end.unwrap_or_else(|| {
            self.error(LexerError::UnterminatedString(start..limit));
            limit
        });
        Spanned {
            value: Token::String(s),
            span: start..end,
        }
    }

    /// Offset just past the closing `quote` for the literal opening at
    /// `start`, or `None` if the literal is never closed. Skips `\x` pairs so
    /// an escaped quote doesn't count.
    fn closing_quote(&self, start: usize, quote: char) -> Option<usize> {
        let body = start + quote.len_utf8();
        let mut it = self.source[body..].char_indices();
        while let Some((i, c)) = it.next() {
            match c {
                '\\' => {
                    it.next();
                }
                _ if c == quote => return Some(body + i + c.len_utf8()),
                _ => {}
            }
        }
        None
    }

    /// Offset of the newline ending the line `at` sits on, or the end of the
    /// source when it is the last line.
    fn line_end(&self, at: usize) -> usize {
        self.source[at..]
            .find('\n')
            .map(|i| at + i)
            .unwrap_or(self.source.len())
    }
}
