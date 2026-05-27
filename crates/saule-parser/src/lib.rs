//! Recursive-descent parser for Saule.
//!
//! The parser consumes a stream of `Spanned<Token>` produced by `saule-lexer`
//! and produces a [`Module`] (a list of [`Spanned<Stmt>`]).
//!
//! Grammar overview (Lua-flavoured, block keywords are `then`/`do`/`end`):
//!
//!   * Expressions follow a standard precedence ladder, lowest at the top:
//!     `or` → `and` → `==`/`!=` → `<`/`<=`/`>`/`>=` → `??` → `..` → `+`/`-`
//!     → `*`/`/`/`%` → unary (`-`, `not`, `#`) → postfix (`.`, `?.`, `[]`,
//!     `(...)`, `:method(...)`, `!`) → primary.
//!   * Statements use keyword-led forms (`if … then … end`, `while … do … end`,
//!     `for v = a, b [, s] do … end`, `for v in iter do … end`,
//!     `repeat … until cond`, `try … catch e: T … end`).
//!   * Declarations: `fn`, `class`, `interface`, `enum`, `import`, `export`.
//!
//! Spans on combined nodes use the start of the leftmost child to the end of
//! the rightmost child so diagnostics highlight the whole construct.
//!
//! ## Module layout
//!
//! The grammar is split across sibling submodules for browsability. Every
//! submodule is just additional `impl Parser` blocks — there's a single
//! [`Parser`] state shared by all of them.
//!
//! | File | Contents |
//! |------|----------|
//! | [`error`] | The [`ParseError`] diagnostic enum |
//! | [`types`] | Type ascriptions, nullables, generic params/args |
//! | [`expr`]  | Expression precedence ladder, primaries, lambdas, params |
//! | [`stmt`]  | Statements, control flow, block helpers |
//! | [`decl`]  | Top-level declarations (`fn`, `class`, `interface`, `enum`, `import`) |

mod decl;
mod error;
mod expr;
mod stmt;
mod types;

pub use error::ParseError;

use saule_ast::{BinOp, Decl, Expr, Module, Spanned, Stmt};
use saule_lexer::Token;
use std::ops::Range;

// ─── Entry point ─────────────────────────────────────────────────────────────

pub fn parse(tokens: Vec<Spanned<Token>>) -> Result<Module, ParseError> {
    let mut p = Parser::new(tokens);
    let mut stmts = Vec::new();
    while !p.is_eof() {
        stmts.push(p.parse_statement()?);
    }
    Ok(Module { stmts })
}

// ─── Parser state ────────────────────────────────────────────────────────────

pub struct Parser {
    tokens: Vec<Spanned<Token>>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Spanned<Token>>) -> Self {
        Self { tokens, pos: 0 }
    }

    // ── Cursor helpers ──────────────────────────────────────────────────────

    pub(crate) fn peek(&self) -> &Spanned<Token> {
        &self.tokens[self.pos]
    }

    pub(crate) fn peek_at(&self, offset: usize) -> &Spanned<Token> {
        let i = (self.pos + offset).min(self.tokens.len() - 1);
        &self.tokens[i]
    }

    pub(crate) fn is_eof(&self) -> bool {
        matches!(self.peek().value, Token::Eof)
    }

    pub(crate) fn advance(&mut self) -> Spanned<Token> {
        let tok = self.tokens[self.pos].clone();
        if !matches!(tok.value, Token::Eof) {
            self.pos += 1;
        }
        tok
    }

    pub(crate) fn check(&self, t: &Token) -> bool {
        std::mem::discriminant(&self.peek().value) == std::mem::discriminant(t)
    }

    /// End offset of the most recently consumed token (or 0 if none).
    pub(crate) fn last_consumed_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.end
        }
    }

    pub(crate) fn eat(&mut self, t: &Token) -> bool {
        if self.check(t) {
            self.advance();
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(
        &mut self,
        t: &Token,
        what: &'static str,
    ) -> Result<Spanned<Token>, ParseError> {
        if self.check(t) {
            Ok(self.advance())
        } else {
            Err(ParseError::Expected {
                expected: what,
                span: self.peek().span.clone(),
            })
        }
    }

    pub(crate) fn expect_ident(
        &mut self,
        what: &'static str,
    ) -> Result<(String, Range<usize>), ParseError> {
        let tok = self.peek().clone();
        if let Token::Identifier(name) = tok.value {
            self.advance();
            Ok((name, tok.span))
        } else {
            Err(ParseError::Expected {
                expected: what,
                span: tok.span,
            })
        }
    }
}

// ─── Free helpers shared across submodules ───────────────────────────────────

pub(crate) fn mk_binary(op: BinOp, lhs: Spanned<Expr>, rhs: Spanned<Expr>) -> Spanned<Expr> {
    let span = lhs.span.start..rhs.span.end;
    Spanned::new(
        Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        span,
    )
}

/// Wraps a `Spanned<Decl>` as a `Spanned<Stmt::Decl(...)>` preserving the span.
pub(crate) fn stmt_decl(d: Spanned<Decl>) -> Spanned<Stmt> {
    let span = d.span.clone();
    Spanned::new(Stmt::Decl(d), span)
}
