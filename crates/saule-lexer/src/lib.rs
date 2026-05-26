mod lexerror;

use lexerror::LexerError;
use saule_ast::Spanned;

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
    End,
    Then,
    Do,
    In,
    As,
    From,
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

    // End of input
    Eof,
}

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

    pub fn tokenize(mut self) -> Result<Vec<Spanned<Token>>, LexerError> {
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
                        loop {
                            match self.chars.next() {
                                Some((_, ']')) => {
                                    if matches!(self.chars.peek(), Some(&(_, ']'))) {
                                        self.chars.next();
                                        break;
                                    }
                                }
                                Some(_) => {}
                                None => {
                                    return Err(LexerError::UnterminatedBlockComment(
                                        start..self.source.len(),
                                    ));
                                }
                            }
                        }
                    } else {
                        // Line comment: until end of line (or EOF).
                        while let Some(&(_, ch)) = self.chars.peek() {
                            if ch == '\n' {
                                break;
                            }
                            self.chars.next();
                        }
                    }
                    continue;
                }
                // Not a comment — fall through so '-' becomes Minus or Arrow.
            }

            let tok = match c {
                '0'..='9' => self.number(start)?,
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

    fn number(&mut self, start: usize) -> Result<Spanned<Token>, LexerError> {
        let mut end = start;
        let mut is_float = false;

        // Integer part.
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

        let text = &self.source[start..end];
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(src: &str) -> Vec<Token> {
        Lexer::new(src)
            .tokenize()
            .unwrap()
            .into_iter()
            .map(|t| t.value)
            .collect()
    }

    #[test]
    fn keywords_and_identifier() {
        assert_eq!(
            lex("local foo"),
            vec![Token::Local, Token::Identifier("foo".into()), Token::Eof]
        );
    }

    #[test]
    fn word_operators() {
        assert_eq!(
            lex("a and b or not c"),
            vec![
                Token::Identifier("a".into()),
                Token::And,
                Token::Identifier("b".into()),
                Token::Or,
                Token::Not,
                Token::Identifier("c".into()),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn integers_and_floats() {
        assert_eq!(
            lex("1 2.5 100"),
            vec![
                Token::Int(1),
                Token::Float(2.5),
                Token::Int(100),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn dot_after_int_is_member_access_when_not_digit() {
        assert_eq!(
            lex("1.foo"),
            vec![
                Token::Int(1),
                Token::Dot,
                Token::Identifier("foo".into()),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn double_dot_is_concat_not_float() {
        assert_eq!(
            lex("1..2"),
            vec![Token::Int(1), Token::DotDot, Token::Int(2), Token::Eof]
        );
    }

    #[test]
    fn ellipsis() {
        assert_eq!(
            lex("fn f(...x: int)"),
            vec![
                Token::Fn,
                Token::Identifier("f".into()),
                Token::LParen,
                Token::Ellipsis,
                Token::Identifier("x".into()),
                Token::Colon,
                Token::Identifier("int".into()),
                Token::RParen,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn line_comment_is_skipped() {
        assert_eq!(
            lex("fn -- comment here\nfoo"),
            vec![Token::Fn, Token::Identifier("foo".into()), Token::Eof]
        );
    }

    #[test]
    fn block_comment_is_skipped() {
        assert_eq!(
            lex("fn --[[ multi\nline ]] foo"),
            vec![Token::Fn, Token::Identifier("foo".into()), Token::Eof]
        );
    }

    #[test]
    fn minus_still_works_after_comment_check() {
        assert_eq!(
            lex("1 - 2"),
            vec![Token::Int(1), Token::Minus, Token::Int(2), Token::Eof]
        );
    }

    #[test]
    fn arrow_and_fat_arrow() {
        assert_eq!(
            lex("-> =>"),
            vec![Token::Arrow, Token::FatArrow, Token::Eof]
        );
    }

    #[test]
    fn null_safety_operators() {
        assert_eq!(
            lex("a?.b ?? c"),
            vec![
                Token::Identifier("a".into()),
                Token::QuestionDot,
                Token::Identifier("b".into()),
                Token::QuestionQuestion,
                Token::Identifier("c".into()),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn string_with_escapes() {
        assert_eq!(
            lex(r#""hi\n\t\"there\"""#),
            vec![Token::String("hi\n\t\"there\"".into()), Token::Eof]
        );
    }

    #[test]
    fn unterminated_string_errors() {
        assert!(matches!(
            Lexer::new("\"oops").tokenize(),
            Err(LexerError::UnterminatedString(_))
        ));
    }

    #[test]
    fn unterminated_block_comment_errors() {
        assert!(matches!(
            Lexer::new("--[[ never closed").tokenize(),
            Err(LexerError::UnterminatedBlockComment(_))
        ));
    }
}
