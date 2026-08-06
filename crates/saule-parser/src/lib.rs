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
//!     `(...)`, `:method(...)`, `do … end`, `!`) → primary.
//!   * A call may carry a **trailing block** — `f(a) do (p) … end`, sugar for
//!     passing a block-bodied lambda as the final argument. Because loop
//!     headers also end in `do`, the form is suppressed while parsing a
//!     `while`/`for` header; see [`Parser::without_trailing_block`].
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
    loop {
        // Semicolons are separators, not statements, so they have to be
        // consumed *before* deciding whether anything is left to parse.
        // Skipping them inside `parse_statement` instead would commit to a
        // statement that isn't there: `local a = 1;` would consume the `;`,
        // find EOF, and report "expected an expression".
        p.skip_semicolons();
        if p.is_eof() {
            break;
        }
        stmts.push(p.parse_statement()?);
    }
    Ok(Module { stmts })
}

// ─── Parser state ────────────────────────────────────────────────────────────

/// Maximum grammatical nesting depth.
///
/// Recursive descent turns source nesting into native-stack recursion, so
/// without a bound a pathological input — `((((…1…))))` — aborts the process
/// with an uncatchable stack overflow rather than reporting a parse error.
/// That matters most in the language server, which parses incomplete input on
/// every keystroke and would take the editor session down with it.
///
/// Set well above anything hand-written (real code rarely passes 30) and well
/// under what the stack can take, so the error is always the binding limit.
pub const MAX_NESTING_DEPTH: u32 = 256;

pub struct Parser {
    tokens: Vec<Spanned<Token>>,
    pos: usize,
    /// Suppresses the trailing-block call form (`f(x) do … end`) while parsing
    /// the header expression of a `while`/`for`, where a following `do` belongs
    /// to the loop rather than to the call. See [`Parser::without_trailing_block`].
    no_trailing_block: bool,
    /// Current grammatical nesting depth — see [`MAX_NESTING_DEPTH`] and
    /// [`Parser::nested`]. Parser state rather than a thread-local because
    /// there is already a `&mut self` threaded through every rule.
    depth: u32,
}

impl Parser {
    pub fn new(tokens: Vec<Spanned<Token>>) -> Self {
        Self {
            tokens,
            pos: 0,
            no_trailing_block: false,
            depth: 0,
        }
    }

    /// Runs `f` with the trailing-block form disabled, restoring the previous
    /// setting afterwards.
    ///
    /// `while queue.pop() do … end` and `for x in items() do … end` end their
    /// header with a call followed by `do`, which is exactly the shape of a
    /// trailing block. The loop wins: inside a header, `do` always closes the
    /// header. Writing a trailing block there is still possible by
    /// parenthesising it — `while (next() do … end) do … end` — because the
    /// flag is cleared for nested parenthesised and bracketed expressions.
    pub(crate) fn without_trailing_block<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        let prev = std::mem::replace(&mut self.no_trailing_block, true);
        let out = f(self);
        self.no_trailing_block = prev;
        out
    }

    /// Runs `f` one grammatical level deeper, refusing past
    /// [`MAX_NESTING_DEPTH`].
    ///
    /// The decrement is unconditional — it runs on the error path too — so a
    /// parser that recovers and keeps going does not carry a stale count into
    /// the rest of the file.
    pub(crate) fn nested<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(ParseError::TooDeep {
                limit: MAX_NESTING_DEPTH,
                span: self.peek().span.clone(),
            });
        }
        self.depth += 1;
        let out = f(self);
        self.depth -= 1;
        out
    }

    /// Runs `f` with the trailing-block form re-enabled. Used by delimited
    /// sub-expressions (`( … )`, `[ … ]`, argument lists, table literals),
    /// where a `do` cannot possibly belong to an enclosing loop header.
    pub(crate) fn with_trailing_block<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        let prev = std::mem::replace(&mut self.no_trailing_block, false);
        let out = f(self);
        self.no_trailing_block = prev;
        out
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

    /// Consumes any run of `;` separators. Callers use this at the point where
    /// they decide whether a block or the module has ended.
    pub(crate) fn skip_semicolons(&mut self) {
        while self.eat(&Token::Semi) {}
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
