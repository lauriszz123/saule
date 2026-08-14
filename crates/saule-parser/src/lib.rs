//! Recursive-descent parser for Saule.
//!
//! The parser consumes a stream of `Spanned<Token>` produced by `saule-lexer`
//! and produces a [`Module`] (a list of [`Spanned<Stmt>`]).
//!
//! Grammar overview (Lua-flavoured, block keywords are `then`/`do`/`end`):
//!
//!   * Expressions follow a standard precedence ladder, lowest at the top:
//!     `or` → `and` → `==`/`!=` → `<`/`<=`/`>`/`>=` → `|` → `~` → `&` →
//!     `<<`/`>>` → `??` → `..` → `+`/`-` → `*`/`/`/`%` → unary (`-`, `not`,
//!     `#`, `~`) → postfix (`.`, `?.`, `[]`, `(...)`, `:method(...)`,
//!     `do … end`, `!`) → primary.
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
//!
//! ## Error recovery
//!
//! The parser does not stop at the first error. It records the diagnostic,
//! repairs or skips the smallest construct it can, and carries on, so a file
//! that is being typed still yields a tree for everything the author has
//! already written. The strategy has three layers, cheapest first:
//!
//! 1. **Missing closers are assumed present.** `end`, `)`, `then`, `do` and
//!    friends go through [`Parser::expect_close`], which records "expected
//!    `end`" and continues as if it had been there. This covers the dominant
//!    mid-typing state — a block whose closing keyword hasn't been written
//!    yet — without discarding anything inside it.
//! 2. **Missing operands become holes.** Where an expression or a type is
//!    required and the tokens can't start one, the parser emits
//!    [`Expr::Error`] / `any` rather than unwinding, so `local pos = ` still
//!    declares `pos` and completion on the next line still sees it.
//! 3. **Everything else resynchronises.** A statement or class member that
//!    fails outright is replaced by a [`Stmt::Error`] hole spanning the
//!    tokens skipped to reach the next statement keyword or block closer.
//!
//! On top of those, one repair operates on the file rather than on a
//! construct. When layer 1 has had to assume an `end`, every declaration
//! below the unterminated block has been parsed *into* it, one scope too
//! deep. [`parse_recover`] then re-reads the file, closing the block where
//! the author evidently meant it to close, on either of two pieces of
//! evidence — see [`Parser::block_ends_here`]:
//!
//! * **Indentation**, an offside rule over declarations. Free, and works on
//!   code no tool has ever seen in a valid state.
//! * **History** — a [`PriorShape`] from this file's last clean parse, passed
//!   in by [`parse_recover_with_prior`]. This is what covers the file that
//!   isn't indented, where there is nothing to be left of.
//!
//! Neither is part of the grammar, so both are gated the same way: the pass
//! runs only on a file an ordinary parse already found a missing `end` in,
//! and its result is kept only if it strands no `end`. A guess can improve a
//! broken file and can never change how valid code parses.
//!
//! Two entry points expose the result. [`parse`] is strict — it hands back
//! the first error and no tree, which is what the interpreter, the CLI and
//! the formatter want, since none of them may act on a guess.
//! [`parse_recover`] hands back both the partial tree and every error, which
//! is what the language server wants, since a diagnostic and a working
//! completion list are not alternatives.
//!
//! The recovery nodes therefore only ever exist in a tree from
//! [`parse_recover`]: `parse` returns `Err` for exactly the inputs that
//! produce them.

mod decl;
mod error;
mod expr;
mod recover;
mod stmt;
mod types;

pub use error::ParseError;
pub use recover::{MAX_ERRORS, PriorShape};

use saule_ast::{BinOp, Decl, Expr, Module, Spanned, Stmt};
use saule_lexer::Token;
use std::ops::Range;

// ─── Entry points ────────────────────────────────────────────────────────────

/// A recovered parse: whatever tree could be built, plus everything that went
/// wrong building it. `errors` is empty exactly when the input was valid.
///
/// Ordered by position, at most one error per token (see
/// [`Parser::record`]) and capped at [`MAX_ERRORS`].
#[derive(Debug)]
pub struct Parsed {
    pub module: Module,
    pub errors: Vec<ParseError>,
    /// Whether a block ran out of input without its `end`. Drives the repair
    /// pass in [`parse_recover`]; not part of the diagnostic surface.
    saw_missing_end: bool,
    /// How many `end` tokens closed nothing. How [`parse_recover`] judges the
    /// repair pass; not part of the diagnostic surface.
    stray_ends: usize,
}

impl Parsed {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Parse strictly: the first error, or a tree with no holes in it.
///
/// The entry point for every consumer that executes, checks or rewrites the
/// code — a partially-guessed tree is worse than no tree when the output is
/// a program's behaviour or the user's file on disk.
pub fn parse(tokens: Vec<Spanned<Token>>) -> Result<Module, ParseError> {
    let parsed = run(tokens);
    match parsed.errors.into_iter().next() {
        // Recovery only begins *after* the first error is recorded, so this
        // is the same error, at the same span, that bailing out would have
        // produced. Callers see no behaviour change.
        Some(first) => Err(first),
        None => Ok(parsed.module),
    }
}

/// Parse tolerantly: always a tree, plus every error found building it.
///
/// The tree may contain [`Expr::Error`] / [`Stmt::Error`] holes wherever the
/// input could not be understood; see the module docs for the recovery rules.
/// Intended for interactive tooling, which has to answer questions about a
/// buffer that is, most of the time, halfway through an edit.
///
/// `source` is the text `tokens` were lexed from, and is used for one thing:
/// working out, when an `end` turns out to be missing, which of the
/// declarations below it were meant to be inside the block and which were
/// stranded there. Passing the wrong text costs accuracy on broken files and
/// nothing else.
///
/// Indentation is the only evidence this form has, and an unindented file has
/// none. [`parse_recover_with_prior`] adds the other half.
pub fn parse_recover(tokens: Vec<Spanned<Token>>, source: &str) -> Parsed {
    parse_recover_with_prior(tokens, source, None)
}

/// [`parse_recover`], told where this file's declarations lived the last time
/// it parsed cleanly.
///
/// Indentation cannot tell a forgotten `end` from a deliberately nested
/// declaration in a file that isn't indented — there is nothing to be left
/// of. Editing history can: a declaration that has sunk into a block it was
/// never in was sunk by a deleted `end`, whatever the whitespace says. Build
/// `prior` with [`PriorShape::of`] from a parse that reported **no errors**;
/// a shape learned from a recovered tree feeds this pass's guesses back into
/// itself.
///
/// A missing, empty or stale shape costs accuracy on broken files and nothing
/// else — it is one of two pieces of evidence, and both are gated behind the
/// same acceptance test.
pub fn parse_recover_with_prior(
    tokens: Vec<Spanned<Token>>,
    source: &str,
    prior: Option<&PriorShape>,
) -> Parsed {
    // The ordinary reading first. If nothing was left unterminated, it is the
    // only reading there is, and the repair pass below has nothing to fix.
    let plain = run(tokens.clone());
    if !plain.saw_missing_end {
        return plain;
    }

    // Something was unterminated, so the declarations after it may have been
    // absorbed into a block they were never meant to be in. Re-read using
    // indentation to close the block early — see
    // [`Parser::block_ends_here`].
    //
    // The test for keeping that reading is **stray `end`s**, not error count.
    // Getting it right usually *raises* the error count, because a run of
    // declarations that each forgot an `end` is reported as several mistakes
    // rather than hidden inside one enormous body. What a wrong guess leaves
    // behind is different and unambiguous: a block closed too early strands
    // the `end` that was meant for it, and an `end` that closes nothing is
    // something a correct reading never produces.
    let repaired = run_repair(tokens, recover::Layout::new(source), prior);
    if repaired.stray_ends <= plain.stray_ends {
        repaired
    } else {
        plain
    }
}

/// One ordinary pass over the tokens, knowing nothing about layout or history.
fn run(tokens: Vec<Spanned<Token>>) -> Parsed {
    finish(Parser::new(tokens))
}

/// One repair pass, with both pieces of evidence available to
/// [`Parser::block_ends_here`].
fn run_repair(
    tokens: Vec<Spanned<Token>>,
    layout: recover::Layout,
    prior: Option<&PriorShape>,
) -> Parsed {
    let mut p = Parser::new(tokens);
    p.layout = Some(layout);
    p.prior = prior.cloned();
    finish(p)
}

fn finish(mut p: Parser) -> Parsed {
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
        stmts.push(p.parse_statement_recovering());
    }
    Parsed {
        module: Module { stmts },
        errors: p.errors,
        saw_missing_end: p.saw_missing_end,
        stray_ends: p.stray_ends,
    }
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
    /// Errors recorded so far, in source order. See [`Parser::record`].
    errors: Vec<ParseError>,
    /// Start offset of the most recently recorded error, used to suppress
    /// the cascade of follow-on complaints a single mistake provokes.
    last_error_pos: Option<usize>,
    /// Depth of speculative parsing — see [`Parser::speculate`]. Nonzero
    /// means "this parse is a probe": recovery is switched off so the probe
    /// can fail, and nothing is recorded.
    speculating: u32,
    /// Line offsets, present only on the repair pass — see
    /// [`Parser::block_ends_here`].
    pub(crate) layout: Option<recover::Layout>,
    /// Where this file's declarations lived at the last clean parse, present
    /// only on the repair pass — see [`Parser::block_ends_here`].
    pub(crate) prior: Option<PriorShape>,
    /// How many blocks enclose the cursor. Compared against [`Self::prior`]
    /// to notice a declaration that has sunk into a block it never used to
    /// be in. Distinct from `depth`, which counts grammatical nesting of
    /// every kind so it can bound recursion.
    pub(crate) block_depth: usize,
    /// Set when a block ran out of input without its `end`. The signal that a
    /// repair pass is worth running; see [`parse_recover`].
    pub(crate) saw_missing_end: bool,
    /// Count of `end` tokens that closed nothing — see [`Parser::skip_one`].
    pub(crate) stray_ends: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Spanned<Token>>) -> Self {
        Self {
            tokens,
            pos: 0,
            no_trailing_block: false,
            depth: 0,
            errors: Vec::new(),
            last_error_pos: None,
            speculating: 0,
            layout: None,
            prior: None,
            block_depth: 0,
            saw_missing_end: false,
            stray_ends: 0,
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

    /// Replace the token under the cursor with two tokens, leaving the cursor
    /// on the first of them.
    ///
    /// The one caller is [`Parser::split_closing_shr`], which turns the `>>`
    /// closing a nested generic back into the two `>`s the grammar asks for.
    /// Rewriting the stream rather than tracking a half-consumed token keeps
    /// every other rule — including [`Parser::speculate`], which rewinds
    /// `pos` — reading an ordinary token sequence.
    pub(crate) fn replace_current(&mut self, first: Spanned<Token>, second: Spanned<Token>) {
        self.tokens[self.pos] = first;
        self.tokens.insert(self.pos + 1, second);
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
