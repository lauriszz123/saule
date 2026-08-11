//! Error recovery: the helpers that let the grammar rules in the sibling
//! modules keep going after a mistake instead of unwinding to the top.
//!
//! See the crate docs for the layered strategy. This module holds the
//! mechanism; the policy — which token is worth assuming, which construct is
//! worth abandoning — lives at the call sites.
//!
//! Three invariants hold everything together, and every function here exists
//! to maintain one of them:
//!
//! * **Progress.** Every recovering loop advances the cursor on every
//!   iteration. A rule that reports an error without consuming anything is
//!   normal (a hole is synthesised in front of the offending token, which is
//!   usually the start of the *next* construct and must not be eaten); it is
//!   the loop's job, not the rule's, to guarantee the cursor moves. See
//!   [`Parser::parse_statement_recovering`].
//! * **Honesty under speculation.** The grammar backtracks in two places, and
//!   a probe that "recovers" its way to success would change what valid code
//!   means. [`Parser::speculate`] switches recovery off for the duration.
//! * **Valid code is untouched.** Every repair here is reached only from an
//!   error path, and the one repair that reads meaning into anything outside
//!   the grammar — [`Parser::block_ends_here`], which weighs indentation and
//!   editing history — is additionally gated on a first pass having already
//!   reported a missing `end`.

use saule_ast::{Expr, Spanned, Stmt, Type};
use saule_lexer::Token;
use std::ops::Range;

use crate::Parser;
use crate::error::ParseError;

/// How many errors are worth reporting from one file.
///
/// A file mid-refactor can be wrong in more ways than anyone will read. The
/// parser keeps building the tree past this point — the tree is the reason
/// recovery exists — and only stops adding to the diagnostic list.
pub const MAX_ERRORS: usize = 64;

/// What a previous, *successful* parse of this file knew about where its
/// declarations lived.
///
/// Indentation is the only in-file evidence of a forgotten `end`, and a file
/// that isn't indented has none — see [`Parser::block_ends_here`]. Editing
/// history has evidence that whitespace doesn't: if `after` was a top-level
/// function one keystroke ago and is suddenly two levels deep, the edit was a
/// deleted `end`, not a restructuring. Nobody nests a function by typing a
/// character somewhere else in the file.
///
/// Only the shallowest depth each name was seen at is kept, which is all the
/// comparison needs and keeps a stale entry from being worse than no entry.
#[derive(Debug, Default, Clone)]
pub struct PriorShape {
    depths: std::collections::HashMap<String, usize>,
}

impl PriorShape {
    /// Record where `module`'s declarations live. Build this only from a
    /// parse that reported no errors — a shape learned from a recovered tree
    /// would feed this pass's own guesses back into it.
    pub fn of(module: &saule_ast::Module) -> Self {
        let mut shape = Self::default();
        shape.walk(&module.stmts, 0);
        shape
    }

    fn walk(&mut self, stmts: &[Spanned<Stmt>], depth: usize) {
        for s in stmts {
            let Stmt::Decl(d) = &s.value else { continue };
            let (name, body) = match &d.value {
                saule_ast::Decl::Function { name, body, .. } => (name, Some(body)),
                saule_ast::Decl::Class { name, .. }
                | saule_ast::Decl::Interface { name, .. }
                | saule_ast::Decl::Enum { name, .. } => (name, None),
                _ => continue,
            };
            let slot = self.depths.entry(name.clone()).or_insert(depth);
            *slot = (*slot).min(depth);
            // Into function bodies, because a nested `fn` is a declaration
            // that can itself be stranded. Not into class members: a method
            // is not a free declaration and can never be one.
            if let Some(body) = body {
                self.walk(body, depth + 1);
            }
        }
    }

    fn depth_of(&self, name: &str) -> Option<usize> {
        self.depths.get(name).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.depths.is_empty()
    }
}

/// Where the lines are, so the parser can ask what column a token is in.
///
/// Saule has no layout rule, so indentation carries no meaning — right up
/// until an `end` is missing, at which point it is the only evidence *inside
/// the file* of what the author meant. See [`Parser::block_ends_here`]. Built
/// from the source only for the repair pass; the ordinary parse never
/// consults it.
pub(crate) struct Layout {
    /// Byte offset of the first character of each line, ascending.
    line_starts: Vec<usize>,
}

impl Layout {
    pub(crate) fn new(source: &str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            source
                .bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| i + 1),
        );
        Self { line_starts }
    }

    fn line_start(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => self.line_starts[i],
            Err(i) => self.line_starts[i.saturating_sub(1)],
        }
    }

    /// Byte column of `offset` within its line. Bytes, not glyphs — the only
    /// use is comparing two columns in the same file, where any consistent
    /// unit does.
    fn column(&self, offset: usize) -> usize {
        offset - self.line_start(offset)
    }
}

impl Parser {
    // ── Recording ───────────────────────────────────────────────────────────

    /// Record a diagnostic, unless it is a follow-on from one already
    /// reported.
    ///
    /// One mistake typically makes several rules fail in turn, all pointing
    /// at the same token: `local x = ` reports a missing expression, and then
    /// whatever the recovering caller tries next fails at the same place. The
    /// rule here is the usual one — **at most one error per token position,
    /// and only ever forwards** — so a diagnostic is kept only if the parser
    /// has made real progress since the last one.
    pub(crate) fn record(&mut self, err: ParseError) {
        if self.speculating > 0 {
            return;
        }
        let at = err.span().start;
        if self.last_error_pos.is_some_and(|last| at <= last) {
            return;
        }
        self.last_error_pos = Some(at);
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(err);
        }
    }

    // ── Speculation ─────────────────────────────────────────────────────────

    /// Run `f` as a probe: recovery is disabled for the duration, and both
    /// the cursor and the error list are restored if it returns `None`.
    ///
    /// The grammar guesses twice — `(` may open an arrow lambda's parameter
    /// list or a parenthesised expression, and `<` may open a generic
    /// instantiation or be a less-than — and resolves both by trying one
    /// reading and rewinding. Without this guard a probe would *recover* its
    /// way through the reading that was supposed to fail: `(1 + 2)` would
    /// report "expected parameter name", patch a hole over it and come back
    /// "yes, a lambda". Recovery must never change how valid code parses.
    pub(crate) fn speculate<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        let saved_pos = self.pos;
        let saved_errors = self.errors.len();
        let saved_last = self.last_error_pos;
        self.speculating += 1;
        let out = f(self);
        self.speculating -= 1;
        if out.is_none() {
            self.pos = saved_pos;
            self.errors.truncate(saved_errors);
            self.last_error_pos = saved_last;
        }
        out
    }

    /// Whether recovery is currently allowed. False inside [`Self::speculate`],
    /// where a rule must be free to fail so the probe can reject its reading.
    pub(crate) fn recovering(&self) -> bool {
        self.speculating == 0
    }

    // ── Layer 1: assume the missing token was there ─────────────────────────

    /// [`Parser::expect`], but recovering: on a mismatch the error is
    /// recorded and `None` returned, and the caller carries on as though the
    /// token had been written.
    ///
    /// Returns `Err` only while speculating, where the caller is a probe that
    /// needs to hear about the failure.
    pub(crate) fn expect_recover(
        &mut self,
        t: &Token,
        what: &'static str,
    ) -> Result<Option<Spanned<Token>>, ParseError> {
        if self.check(t) {
            return Ok(Some(self.advance()));
        }
        let err = ParseError::Expected {
            expected: what,
            span: self.peek().span.clone(),
        };
        if !self.recovering() {
            return Err(err);
        }
        self.record(err);
        Ok(None)
    }

    /// [`Self::expect_recover`] for a token that *closes* something, yielding
    /// the end offset the enclosing construct's span should use: the closer's
    /// own end, or — when it was never written — as far as parsing got.
    ///
    /// This is the single highest-value recovery in the parser. Code is
    /// written top-down, so the `end` of the block you are inside is almost
    /// always the thing that hasn't been typed yet; treating that as fatal
    /// throws away the entire declaration the cursor is in, which is exactly
    /// the declaration the editor is being asked about.
    pub(crate) fn expect_close(
        &mut self,
        t: &Token,
        what: &'static str,
    ) -> Result<usize, ParseError> {
        Ok(match self.expect_recover(t, what)? {
            Some(tok) => tok.span.end,
            None => {
                if matches!(t, Token::End) {
                    // The signal `parse_recover` needs to decide whether the
                    // declarations below this block belong to it.
                    self.saw_missing_end = true;
                }
                self.last_consumed_end()
            }
        })
    }

    /// [`Parser::expect_ident`], but recovering: an empty name stands in for
    /// the identifier that wasn't there.
    ///
    /// Nothing is consumed on failure — the token in front is usually the
    /// start of the next construct (`fn` on the following line), and eating
    /// it would cost more than the name is worth. Downstream tooling can
    /// recognise the placeholder by `name.is_empty()`; no real identifier can
    /// be empty.
    ///
    /// **Not for a name that gates a block.** `fn`, `class`, `interface` and
    /// `enum` read their names with the strict [`Parser::expect_ident`],
    /// because inventing one commits the parser to a body running to the next
    /// `end` — and that body then swallows every declaration after it, so one
    /// bad line costs the rest of the file. Failing there instead lets the
    /// enclosing loop resynchronise. Every other name (locals, parameters,
    /// fields, `import` lists, loop variables, `.member`) opens nothing, and
    /// recovers.
    pub(crate) fn expect_ident_recover(
        &mut self,
        what: &'static str,
    ) -> Result<(String, Range<usize>), ParseError> {
        let tok = self.peek().clone();
        if let Token::Identifier(name) = tok.value {
            self.advance();
            return Ok((name, tok.span));
        }
        let err = ParseError::Expected {
            expected: what,
            span: tok.span.clone(),
        };
        if !self.recovering() {
            return Err(err);
        }
        self.record(err);
        Ok((String::new(), tok.span.start..tok.span.start))
    }

    // ── Layer 2: holes where an operand was required ────────────────────────

    /// The expression to use where one was required but the tokens in front
    /// cannot start one.
    ///
    /// Consumes nothing, deliberately: in the shape this is written for —
    ///
    /// ```text
    /// local pos =
    /// local size = 2
    /// ```
    ///
    /// — the token that "isn't an expression" is the `local` beginning the
    /// next statement. Eating it would turn one broken line into two. The
    /// statement loops carry the progress guard that makes a zero-width hole
    /// safe.
    pub(crate) fn error_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let span = self.peek().span.clone();
        let err = ParseError::Expected {
            expected: "an expression",
            span: span.clone(),
        };
        if !self.recovering() {
            return Err(err);
        }
        self.record(err);
        Ok(Spanned::new(Expr::Error, span.start..span.start))
    }

    /// The type to use where one was required but couldn't be parsed.
    ///
    /// `any` rather than a dedicated error type: the type system already has
    /// a word for "could be anything", every pass already handles it, and it
    /// makes the binding *usable* — `local n: = 1` still hovers, still
    /// completes, and produces no second diagnostic on top of the parse
    /// error.
    pub(crate) fn error_type(&mut self, what: &'static str) -> Result<Type, ParseError> {
        let err = ParseError::Expected {
            expected: what,
            span: self.peek().span.clone(),
        };
        if !self.recovering() {
            return Err(err);
        }
        self.record(err);
        Ok(Type::Named("any".to_string()))
    }

    // ── The forgotten `end` ─────────────────────────────────────────────────

    /// Whether the token in front should be read as closing the block being
    /// parsed, because its `end` was never written.
    ///
    /// The shape this exists for is the commonest way a Saule file is broken:
    ///
    /// ```text
    /// fn before()
    ///     local a = 1        <- the body sits at column 4
    ///                        <- and the `end` is missing
    /// fn after()             <- column 0: this was never meant to be nested
    ///     local b = 2
    /// end
    /// ```
    ///
    /// Without this, `after` is parsed into `before`'s body and every
    /// declaration below it is scoped one level too deep.
    ///
    /// Two independent pieces of evidence, either of which is enough. Both
    /// apply only to declaration keywords — what a forgotten `end` actually
    /// strands — and only to a token leading its own line, since a
    /// declaration written mid-line is nobody's idea of a top-level one.
    ///
    /// 1. **Indentation.** An offside rule: a declaration to the left of this
    ///    block's body has left the block. Free, and works on code the editor
    ///    has never seen in a valid state — but silent on a file that isn't
    ///    indented, where there is nothing to be left of.
    /// 2. **History.** [`PriorShape`] from the last clean parse of this same
    ///    file. If the declaration in front used to live shallower than the
    ///    block now enclosing it, it has sunk, and the only edit that sinks a
    ///    declaration without touching it is a deleted `end`. This is what
    ///    covers the unindented file, the empty body, and the declaration
    ///    that happens to sit at exactly the body's column.
    ///
    /// **Neither is ever consulted outside the repair pass.** Indentation is
    /// not part of the grammar, so `fn nested()` written a little to the left
    /// of its siblings is legal and rule 1 would break it; history is stale
    /// by construction. [`crate::parse_recover`] runs this pass only after an
    /// ordinary parse has already reported a missing `end`, and keeps the
    /// result only if it strands no `end` the ordinary reading consumed — so
    /// a guess can improve a file that is already broken and cannot touch one
    /// that isn't.
    pub(crate) fn block_ends_here(&self, body_col: Option<usize>) -> bool {
        if self.layout.is_none() || !starts_declaration(&self.peek().value) || !self.at_line_start()
        {
            return false;
        }
        self.dedented_past(body_col) || self.sunk_below_prior_depth()
    }

    /// Evidence 1: the declaration in front starts left of this block's body.
    fn dedented_past(&self, body_col: Option<usize>) -> bool {
        let (Some(layout), Some(body_col)) = (&self.layout, body_col) else {
            return false;
        };
        layout.column(self.peek().span.start) < body_col
    }

    /// Evidence 2: the declaration in front used to live shallower than the
    /// block it is about to be parsed into.
    fn sunk_below_prior_depth(&self) -> bool {
        let Some(prior) = &self.prior else {
            return false;
        };
        self.upcoming_decl_name()
            .and_then(|name| prior.depth_of(name))
            .is_some_and(|was| was < self.block_depth)
    }

    /// The name of the declaration the tokens in front spell, if they spell
    /// one: `fn f`, `class C`, `export fn f`, and so on.
    ///
    /// `local fn f` is deliberately absent. It declares a function *scoped to
    /// the enclosing block*, so finding one inside a block is not evidence of
    /// anything having gone wrong.
    fn upcoming_decl_name(&self) -> Option<&str> {
        let after_export = matches!(self.peek().value, Token::Export) as usize;
        let keyword = &self.peek_at(after_export).value;
        let named_by_keyword = matches!(
            keyword,
            Token::Fn | Token::Class | Token::Interface | Token::Enum
        );
        // `export name: T = value` names itself; everything else is preceded
        // by its keyword.
        let name_at = if named_by_keyword {
            after_export + 1
        } else if after_export == 1 {
            1
        } else {
            return None;
        };
        match &self.peek_at(name_at).value {
            Token::Identifier(name) => Some(name),
            _ => None,
        }
    }

    /// The column of the token in front, or `None` outside the repair pass.
    pub(crate) fn line_col(&self) -> Option<usize> {
        self.layout
            .as_ref()
            .map(|l| l.column(self.peek().span.start))
    }

    /// Whether the token in front is the first one on its line — i.e. nothing
    /// but whitespace and comments precedes it there.
    pub(crate) fn at_line_start(&self) -> bool {
        self.pos == 0
            || self.layout.as_ref().is_some_and(|l| {
                self.tokens[self.pos - 1].span.end <= l.line_start(self.peek().span.start)
            })
    }

    /// A span from `start` to wherever parsing has reached — never inverted.
    ///
    /// Recovery can complete a node without consuming a single token, which
    /// would leave `start` past `last_consumed_end()` and hand tooling a
    /// range that runs backwards through the source.
    pub(crate) fn span_to_here(&self, start: usize) -> Range<usize> {
        start..self.last_consumed_end().max(start)
    }

    // ── Layer 3: resynchronise ──────────────────────────────────────────────

    /// Parse one statement, converting an unrecoverable failure into a
    /// [`Stmt::Error`] hole spanning the tokens skipped to get back on track.
    ///
    /// **This is the only place the progress invariant is enforced**, and it
    /// covers the success path too: a statement can legitimately parse
    /// without consuming anything once holes exist (`then` at statement
    /// position becomes `Stmt::Expr(Expr::Error)`), and a block loop around a
    /// zero-width statement would spin forever.
    pub(crate) fn parse_statement_recovering(&mut self) -> Spanned<Stmt> {
        let start = self.peek().span.start;
        let before = self.pos;
        let stmt = match self.parse_statement() {
            Ok(s) => s,
            Err(err) => {
                self.record(err);
                // Nothing was consumed, so the token in front is the one that
                // failed. Skipping it before synchronising is what keeps a
                // rule that reports-without-consuming from stalling here.
                if self.pos == before {
                    self.skip_one();
                }
                self.synchronize();
                Spanned::new(Stmt::Error, start..self.last_consumed_end().max(start))
            }
        };
        if self.pos == before {
            self.skip_one();
        }
        stmt
    }

    /// Drop the token in front as unparseable.
    ///
    /// Counts it when it is an `end`, because an `end` that closes nothing is
    /// the fingerprint of a block having been closed too early — which is
    /// exactly the mistake the dedent repair can make, and how
    /// [`crate::parse_recover`] decides whether to trust it.
    fn skip_one(&mut self) {
        if self.check(&Token::End) {
            self.stray_ends += 1;
        }
        self.advance();
    }

    /// Panic-mode resynchronisation: skip tokens until one that could begin a
    /// statement, or one that closes the enclosing block.
    ///
    /// Stopping at block closers as well as statement starts is what keeps a
    /// mistake inside a function from swallowing the functions after it: the
    /// `end` is left for the enclosing rule to consume, so the declaration
    /// closes where the author meant it to.
    pub(crate) fn synchronize(&mut self) {
        while !self.is_eof() && !starts_statement(&self.peek().value) && !self.at_block_terminator()
        {
            self.advance();
        }
    }

    /// [`Self::synchronize`] for the body of a `class` / `interface` / `enum`,
    /// whose members are not statements: stop at anything that could begin a
    /// member, or at the `end` that closes the declaration.
    pub(crate) fn synchronize_member(&mut self) {
        while !self.is_eof()
            && !matches!(
                self.peek().value,
                Token::Fn
                    | Token::Static
                    | Token::Local
                    | Token::Identifier(_)
                    | Token::End
                    | Token::Class
                    | Token::Interface
                    | Token::Enum
                    | Token::Export
            )
        {
            self.advance();
        }
    }
}

/// Whether a token begins a declaration — the keywords a forgotten `end`
/// strands inside the block above them. See
/// [`Parser::block_ends_here`].
fn starts_declaration(t: &Token) -> bool {
    matches!(
        t,
        Token::Fn | Token::Class | Token::Interface | Token::Enum | Token::Import | Token::Export
    )
}

/// Whether a token can begin a statement — the synchronisation set for
/// [`Parser::synchronize`].
fn starts_statement(t: &Token) -> bool {
    matches!(
        t,
        Token::Local
            | Token::If
            | Token::While
            | Token::Repeat
            | Token::For
            | Token::Try
            | Token::Return
            | Token::Throw
            | Token::Break
            | Token::Continue
            | Token::Fn
            | Token::Class
            | Token::Interface
            | Token::Enum
            | Token::Import
            | Token::Export
            | Token::Static
            | Token::Semi
    )
}
