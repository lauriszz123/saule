mod error;
mod token;

pub use error::LexerError;
use saule_ast::Spanned;
pub use token::Token;

pub struct Lexer<'src> {
    source: &'src str,
    chars: std::iter::Peekable<std::str::CharIndices<'src>>,
}

impl<'src> Lexer<'src> {
    pub fn new(source: &'src str) -> Self {
        Self {
            source,
            chars: source.char_indices().peekable(),
        }
    }

    /// Tokenize and discard comments. The default entry point: matches
    /// the lexer's historical behaviour and keeps the parser unaware of
    /// trivia.
    pub fn tokenize(self) -> Result<Vec<Spanned<Token>>, LexerError> {
        let all = self.tokenize_with_trivia()?;
        Ok(all
            .into_iter()
            .filter(|t| !matches!(t.value, Token::LineComment(_) | Token::BlockComment(_)))
            .collect())
    }

    /// Tokenize and preserve `--` line / `--[[ … ]]` block comments as
    /// `Token::LineComment` / `Token::BlockComment` with their full span
    /// (including delimiters). Used by the formatter to round-trip
    /// comments; the parser still goes through [`Self::tokenize`].
    pub fn tokenize_with_trivia(mut self) -> Result<Vec<Spanned<Token>>, LexerError> {
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
                        let body_end;
                        loop {
                            match self.chars.next() {
                                Some((i, ']')) if matches!(self.chars.peek(), Some(&(_, ']'))) => {
                                    self.chars.next();
                                    body_end = i; // first `]` of the `]]` pair
                                    break;
                                }
                                Some(_) => {}
                                None => {
                                    return Err(LexerError::UnterminatedBlockComment(
                                        start..self.source.len(),
                                    ));
                                }
                            }
                        }
                        let text = self.source[body_start..body_end].to_string();
                        let span_end = body_end + 2; // include the `]]`
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
                '0'..='9' => self.number(start)?,
                _ if dot_starts_float => self.number(start)?,
                'a'..='z' | 'A'..='Z' | '_' => self.ident_or_keyword(start),
                '"' => self.string(start)?,
                _ => self.symbol(start)?,
            };

            out.push(tok);
        }

        let end = self.source.len();
        out.push(Spanned {
            value: Token::Eof,
            span: end..end,
        });

        Ok(out)
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
    fn number(&mut self, start: usize) -> Result<Spanned<Token>, LexerError> {
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
        if let Some(&(sfx_i, sfx)) = self.chars.peek() {
            if sfx == 'f' || sfx == 'F' {
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
        }

        let text = &self.source[start..digits_end];
        let tok = if is_float {
            Token::Float(
                text.parse()
                    .map_err(|_| LexerError::BadNumber(start..end))?,
            )
        } else {
            Token::Int(
                text.parse()
                    .map_err(|_| LexerError::BadNumber(start..end))?,
            )
        };

        Ok(Spanned {
            value: tok,
            span: start..end,
        })
    }

    /// Lex `0x…` / `0b…` after the leading `0`, with `radix` already chosen.
    ///
    /// Underscores are allowed as digit separators (`0xFF_FF`), because the
    /// literals this form is for — colour values, bit masks, codepoints — are
    /// exactly the ones that are hard to read in one run.
    fn radix_number(&mut self, start: usize, radix: u32) -> Result<Spanned<Token>, LexerError> {
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

        // `0x` with nothing after it is not a number.
        if digits.is_empty() {
            return Err(LexerError::BadNumber(start..end));
        }

        let value =
            i64::from_str_radix(&digits, radix).map_err(|_| LexerError::BadNumber(start..end))?;

        Ok(Spanned {
            value: Token::Int(value),
            span: start..end,
        })
    }

    fn symbol(&mut self, start: usize) -> Result<Spanned<Token>, LexerError> {
        let (_, c) = self.chars.next().unwrap();
        let (tok, len) = match c {
            '+' => (Token::Plus, 1),
            '-' => match self.chars.peek() {
                Some(&(_, '>')) => {
                    self.chars.next();
                    (Token::Arrow, 2)
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
            '*' => (Token::Star, 1),
            '/' => (Token::Slash, 1),
            '%' => (Token::Percent, 1),
            '#' => (Token::Hash, 1),
            _ => return Err(LexerError::Unexpected(start..start + c.len_utf8())),
        };
        Ok(Spanned {
            value: tok,
            span: start..start + len,
        })
    }

    fn string(&mut self, start: usize) -> Result<Spanned<Token>, LexerError> {
        self.chars.next(); // consume opening "
        let mut s = String::new();
        let end;
        loop {
            match self.chars.next() {
                Some((i, '"')) => {
                    end = i + 1;
                    break;
                }
                Some((_, '\\')) => match self.chars.next() {
                    Some((_, 'n')) => s.push('\n'),
                    Some((_, 't')) => s.push('\t'),
                    Some((_, 'r')) => s.push('\r'),
                    Some((_, '0')) => s.push('\0'),
                    Some((_, '\\')) => s.push('\\'),
                    Some((_, '"')) => s.push('"'),
                    Some((i, c)) => return Err(LexerError::BadEscape(i..i + c.len_utf8())),
                    None => return Err(LexerError::UnterminatedString(start..self.source.len())),
                },
                Some((_, c)) => s.push(c),
                None => return Err(LexerError::UnterminatedString(start..self.source.len())),
            }
        }
        Ok(Spanned {
            value: Token::String(s),
            span: start..end,
        })
    }
}
