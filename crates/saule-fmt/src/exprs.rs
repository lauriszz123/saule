//! Printing expressions, including the inline-vs-broken decisions for
//! call arguments, table literals and `when(...)` pipelines.

use saule_ast::{CallArg, Expr, LambdaBody, PipeStage, Spanned, TableEntry, UnaryOp};
use std::ops::Range;

use super::*;

impl<'a> Printer<'a> {
    /// Print an expression. `parent_prec` is the binding strength of the
    /// surrounding context; if our own precedence is lower we wrap in
    /// parens to preserve grouping.
    pub(crate) fn expr(&mut self, e: &Spanned<Expr>, parent_prec: u8) {
        let outer_end = e.span.end;
        match &e.value {
            Expr::Int(n) => self.writef(format_args!("{n}")),
            Expr::Float(f) => {
                let s = self.float_lit(*f, &e.span);
                self.write(&s);
            }
            Expr::Bool(b) => self.write(if *b { "true" } else { "false" }),
            Expr::Str(s) => {
                let lit = self.str_lit(s, &e.span);
                self.write(&lit);
            }
            Expr::Nil => self.write("nil"),
            Expr::Ident(n) => self.write(n),
            Expr::Self_ => self.write("self"),

            // Recovery holes never reach the formatter: every entry point
            // parses strictly and declines to format a file that didn't.
            // Printing nothing is the one choice that can't invent source
            // text if that ever stops being true.
            Expr::Error => {}

            Expr::Unary { op, rhs } => {
                let (sym, space) = match op {
                    UnaryOp::Neg => ("-", false),
                    UnaryOp::Not => ("not", true),
                    UnaryOp::Len => ("#", false),
                };
                // A unary is not the tightest thing in the grammar: `^` and
                // `as` both bind above it. So it needs the same treatment as
                // a binary — parenthesise where the context binds tighter, and
                // where the author asked for it.
                let parens = UNARY_PREC < parent_prec || self.was_grouped(parent_prec, &e.span);
                if parens {
                    self.write("(");
                }
                self.write(sym);
                if space {
                    self.write(" ");
                }
                // The operand keeps `MAX_PREC`: `-a ^ b` already means
                // `-(a ^ b)`, so printing the inner `^` bare would be correct
                // but relies on the reader knowing that. Parenthesising it is
                // the same call as everywhere else in this file.
                self.expr(rhs, MAX_PREC);
                if parens {
                    self.write(")");
                }
            }
            Expr::Binary { op, lhs, rhs } => {
                let (p, right_assoc) = bin_prec(*op);
                // Two reasons to parenthesise: precedence demands it, or the
                // author wrote parentheses here and precedence alone is not a
                // good enough reason to take them away.
                let parens = p < parent_prec || self.was_grouped(parent_prec, &e.span);
                if parens {
                    self.write("(");
                }
                let left_min = if right_assoc { p + 1 } else { p };
                let right_min = if right_assoc { p } else { p + 1 };
                self.expr(lhs, left_min);
                self.writef(format_args!(" {} ", bin_sym(*op)));
                self.expr(rhs, right_min);
                if parens {
                    self.write(")");
                }
            }

            Expr::Member { obj, name } => {
                self.expr(obj, MAX_PREC);
                self.write(".");
                self.write(name);
            }
            Expr::SafeMember { obj, name } => {
                self.expr(obj, MAX_PREC);
                self.write("?.");
                self.write(name);
            }
            Expr::Index { obj, index } => {
                self.expr(obj, MAX_PREC);
                self.write("[");
                self.expr(index, 0);
                self.write("]");
            }
            Expr::Call { callee, args } => {
                // A trailing block prints back as one — but only when that is
                // how it was written. `f(a, fn() … end)` and `f(a) do … end`
                // parse to the same tree, so moving the lambda out of the
                // parentheses is a rewrite, not a reformat: it changes which
                // parameter the argument visibly targets, and the author may
                // have put the lambda inside the parens precisely because a
                // later parameter follows it. Which form was written is
                // recovered from the lambda's own span — see
                // [`Self::is_written_as_do_block`].
                //
                // Suppressed under `force_inline`, where the caller is
                // measuring a single-line rendering and a block can't fit.
                let trailing = if self.force_inline {
                    None
                } else {
                    trailing_block_arg(args).filter(|t| self.is_written_as_do_block(t))
                };
                self.expr(callee, MAX_PREC);
                self.write("(");
                self.call_args(trailing.map_or(args, |t| t.leading));
                self.write(")");
                if let Some(t) = trailing {
                    self.write(" do");
                    // The parser only looks for a return type after a
                    // parameter list, so `-> T` needs `()` even when there
                    // are no parameters.
                    if !t.params.is_empty() || t.return_ty.is_some() {
                        self.write(" (");
                        self.params(t.params);
                        self.write(")");
                    }
                    if let Some(rt) = t.return_ty {
                        self.write(" -> ");
                        self.ty(rt);
                    }
                    self.newline();
                    self.block(t.body, outer_end);
                    self.write("end");
                }
            }
            Expr::ForceUnwrap(inner) => {
                self.expr(inner, MAX_PREC);
                self.write("!");
            }
            Expr::Cast { value, ty } => {
                // `as` sits between the binary operators and the postfix
                // chain, so it needs parentheses in a postfix context and
                // none inside a binary one: `(x as integer)!` must keep
                // its parens (dropping them yields `x as integer!`, which
                // doesn't parse), while `x as integer != nil` is already
                // unambiguous.
                let parens = CAST_PREC < parent_prec || self.was_grouped(parent_prec, &e.span);
                if parens {
                    self.write("(");
                }
                self.expr(value, MAX_PREC);
                self.write(" as ");
                self.ty(ty);
                if parens {
                    self.write(")");
                }
            }

            Expr::Table(entries) => {
                if entries.is_empty() {
                    self.write("{}");
                } else {
                    // Mirrors the `when(...)` layout policy: inline by default,
                    // multi-line when the inline form overflows the width
                    // target OR the user already broke an entry onto its own
                    // line in the source.
                    let start_col = self.current_column();
                    let inline = self.render_table_inline(entries);
                    let force_ml = self.table_has_source_break(entries);
                    let too_wide = start_col + inline.len() > self.max_width();
                    if self.force_inline || (!force_ml && !too_wide) {
                        self.write(&inline);
                    } else {
                        self.write("{");
                        self.indent += 1;
                        for ent in entries {
                            self.newline();
                            self.write_table_entry(ent);
                            self.write(",");
                        }
                        self.indent -= 1;
                        self.newline();
                        self.write("}");
                    }
                }
            }

            Expr::Lambda {
                params,
                return_ty,
                body,
            } => match body {
                // Arrow lambdas: `x => expr` (single any-typed param, no
                // return type) or `(params) => expr`. The parser has no
                // `fn(...) => expr` form, so block bodies must use `fn`.
                LambdaBody::Expr(e) => {
                    if is_bare_arrow_param(params, return_ty) {
                        self.write(&params[0].name);
                    } else {
                        self.write("(");
                        self.params(params);
                        self.write(")");
                        if let Some(rt) = return_ty {
                            self.write(" -> ");
                            self.ty(rt);
                        }
                    }
                    self.write(" => ");
                    self.expr(e, 0);
                }
                LambdaBody::Block(stmts) => {
                    self.write("fn(");
                    self.params(params);
                    self.write(")");
                    if let Some(rt) = return_ty {
                        self.write(" -> ");
                        self.ty(rt);
                    }
                    self.newline();
                    self.block(stmts, outer_end);
                    self.write("end");
                }
            },

            Expr::Match { scrutinee, arms } => {
                self.write("match ");
                self.expr(scrutinee, 0);
                self.newline();
                self.indent += 1;
                if let Some(first) = arms.first() {
                    self.last_pos = self
                        .last_pos
                        .max(line_start_in_source(self.source, first.span.start));
                }
                for (i, a) in arms.iter().enumerate() {
                    self.drain_before(a.span.start);
                    // Blank line between arms when the source had one
                    // (≥ 2 newlines between the previous arm's end and
                    // this arm's start, after accounting for any
                    // comments drained in the gap).
                    if i > 0 {
                        let prev_end = self.last_pos.max(arms[i - 1].span.end);
                        if self.newlines_in_source(prev_end, a.span.start) >= 2 {
                            self.blank_line();
                        }
                    }
                    self.match_arm(a);
                    self.last_pos = self.last_pos.max(a.span.end);
                    self.try_trailing(a.span.end);
                    self.newline();
                }
                self.drain_before(outer_end);
                self.indent -= 1;
                self.write("end");
            }

            // `when(source):stage1(args):stage2(args)…` — colon pipeline.
            //
            // Layout:
            //   * Inline by default when the whole chain fits within the
            //     width target from the current column. Reads as
            //     `when(x):a():b()`.
            //   * Multi-line when (a) the inline form is too wide, or
            //     (b) the source already broke before *any* `:` — the
            //     user's intent wins, so explicit multi-line stays
            //     multi-line even on tiny chains.
            //   * Multi-line indent: every `:` is aligned to the column
            //     where the `w` of `when` lives, so the method names
            //     line up just past the keyword:
            //
            //         local r: integer = when({1, 2, 3})
            //                            :a()
            //                            :b()
            Expr::Pipe { source, stages } => {
                let when_col = self.current_column();
                let force_ml = self.pipe_has_source_break(source, stages);
                let inline = self.render_pipe_inline(source, stages);
                let too_wide = when_col + inline.len() > self.max_width();
                if self.force_inline || (!force_ml && !too_wide) {
                    self.write(&inline);
                } else {
                    self.write("when(");
                    self.expr(source, 0);
                    self.write(")");
                    for stage in stages {
                        self.newline();
                        // Skip the indent prefix the next `write` would
                        // emit and lay down `when_col` spaces directly so
                        // `:` lands exactly under the original `w`.
                        self.needs_indent = false;
                        for _ in 0..when_col {
                            self.out.push(' ');
                        }
                        self.write(":");
                        self.write(&stage.name);
                        self.write("(");
                        self.call_args(&stage.args);
                        self.write(")");
                    }
                }
            }
        }
    }

    // ── `when(...)` pipeline layout helpers ────────────────────────────────

    pub(crate) fn expr_list(&mut self, es: &[Spanned<Expr>]) {
        for (i, e) in es.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.expr(e, 0);
        }
    }

    /// `true` when the user broke any `:stage()` onto its own line in the
    /// original source. We honour that — once it's multi-line in the
    /// source it stays multi-line in the output, even if the chain is
    /// short enough to fit inline.
    pub(crate) fn pipe_has_source_break(
        &self,
        source: &Spanned<Expr>,
        stages: &[PipeStage],
    ) -> bool {
        if self.source.is_empty() {
            return false;
        }
        // Gap between `when(source)` and the first `:`.
        if let Some(first) = stages.first()
            && self.source_range_has_newline(source.span.end..first.span.start)
        {
            return true;
        }
        // Gaps between successive stages.
        stages
            .windows(2)
            .any(|pair| self.source_range_has_newline(pair[0].span.end..pair[1].span.start))
    }

    /// Render `when(source):stage1(args):stage2(args)…` into a string
    /// without touching `self.out`. The result is the inline form; the
    /// caller compares its length against the available room to decide
    /// whether to commit it or fall back to the multi-line layout.
    pub(crate) fn render_pipe_inline(
        &self,
        source: &Spanned<Expr>,
        stages: &[PipeStage],
    ) -> String {
        // Comments inside the chain are ignored for the size estimate; they
        // only ever fire in the real `self` printer.
        let mut sub = self.sub_printer();
        sub.write("when(");
        sub.expr(source, 0);
        sub.write(")");
        for stage in stages {
            sub.write(":");
            sub.write(&stage.name);
            sub.write("(");
            sub.call_args(&stage.args);
            sub.write(")");
        }
        sub.out
    }

    // ── Table-literal layout helpers ───────────────────────────────────────

    /// Render `{ entry, entry, ... }` into a single-line string using a
    /// sub-printer. Used to decide whether the inline form fits within the
    /// width target before committing to a layout.
    pub(crate) fn render_table_inline(&self, entries: &[TableEntry]) -> String {
        let mut sub = self.sub_printer();
        sub.write("{");
        for (i, ent) in entries.iter().enumerate() {
            if i > 0 {
                sub.write(", ");
            }
            sub.write_table_entry(ent);
        }
        sub.write("}");
        sub.out
    }

    /// True when the user broke any pair of consecutive entries onto
    /// separate lines in the source. We honour that — once it's
    /// multi-line in the source it stays multi-line on output, even
    /// when the inline form would fit.
    pub(crate) fn table_has_source_break(&self, entries: &[TableEntry]) -> bool {
        if self.source.is_empty() || entries.len() < 2 {
            return false;
        }
        entries
            .windows(2)
            .any(|pair| self.source_range_has_newline(entry_end(&pair[0])..entry_start(&pair[1])))
    }

    /// Emit a single table entry — shared by inline and multi-line layouts.
    pub(crate) fn write_table_entry(&mut self, ent: &TableEntry) {
        match ent {
            TableEntry::Positional(e) => self.expr(e, 0),
            TableEntry::Field { key, value } => {
                if let Expr::Str(s) = &key.value {
                    if is_ident(s) {
                        self.write(s);
                    } else {
                        let lit = self.str_lit(s, &key.span);
                        self.write(&lit);
                    }
                } else {
                    self.expr(key, 0);
                }
                self.write(": ");
                self.expr(value, 0);
            }
        }
    }

    /// Emit `arg, arg, …` between an already-written `(` and the `)` the
    /// caller writes next.
    ///
    /// Inline when the whole list plus its closing paren fits inside the width
    /// target; otherwise one argument per line at one extra indent level, so
    /// the closing paren lands back at the caller's indent.
    ///
    /// Note the comma is a *separator* here, not a terminator: unlike a table
    /// literal, `parse_call_args` requires an argument after every comma, so a
    /// trailing one would make the formatter's own output unparseable.
    pub(crate) fn call_args(&mut self, args: &[CallArg]) {
        if args.is_empty() {
            return;
        }
        let start_col = self.current_column();
        let inline = self.render_call_args_inline(args);
        // `+ 1` reserves the closing paren.
        if self.force_inline || start_col + inline.len() < self.max_width() {
            self.write(&inline);
            return;
        }

        self.indent += 1;
        for (i, a) in args.iter().enumerate() {
            self.newline();
            self.write_call_arg(a);
            if i + 1 < args.len() {
                self.write(",");
            }
        }
        self.indent -= 1;
        self.newline();
    }

    /// Render an argument list on one line, for width measurement.
    pub(crate) fn render_call_args_inline(&self, args: &[CallArg]) -> String {
        let mut sub = self.sub_printer();
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                sub.write(", ");
            }
            sub.write_call_arg(a);
        }
        sub.out
    }

    /// Emit a single argument — shared by the inline and multi-line layouts.
    pub(crate) fn write_call_arg(&mut self, a: &CallArg) {
        match a {
            CallArg::Positional(e) => self.expr(e, 0),
            CallArg::Named { name, value } => {
                self.writef(format_args!("{name}: "));
                self.expr(value, 0);
            }
        }
    }

    /// Render a float literal, preferring the exact text the author wrote.
    ///
    /// The AST only carries the parsed `f64`, so `0f`, `.5` and `1.50` would
    /// otherwise all come back as the canonical `0.0` / `0.5` / `1.5`. When the
    /// original source is available we reuse the literal's own span verbatim,
    /// provided it still reads back as the same value; [`format_float`] covers
    /// the source-less path (`format_module`) and any span that doesn't match.
    pub(crate) fn float_lit(&self, f: f64, span: &Range<usize>) -> String {
        match self.source.get(span.clone()) {
            Some(raw) if float_text_matches(raw, f) => raw.to_string(),
            _ => format_float(f),
        }
    }

    /// Render a string literal, keeping whichever quote style it was written
    /// with.
    ///
    /// `Token::String` carries only the decoded value, so the delimiter is
    /// gone by the time the AST exists. Rather than widen the token and every
    /// consumer of `Expr::Str` to carry it, read it back out of the source at
    /// the literal's span — the same trick [`Self::float_lit`] uses to keep
    /// `1f` from being reprinted as `1.0`.
    ///
    /// Falls back to picking a delimiter when there is no source to consult
    /// (`format_module`) or when the span isn't a quoted literal at all — a
    /// `{ key: value }` table entry parses to `Expr::Str` whose span covers a
    /// bare identifier.
    pub(crate) fn str_lit(&self, s: &str, span: &Range<usize>) -> String {
        quote_str_with(s, self.source_quote(span))
    }

    /// The quote character at `span`'s first byte, if it is one.
    fn source_quote(&self, span: &Range<usize>) -> Option<char> {
        let c = self.source.get(span.start..)?.chars().next()?;
        (c == '"' || c == '\'').then_some(c)
    }

    /// Whether the author parenthesised the operand at `span`.
    ///
    /// The parser discards grouping — `(a + b)` and `a + b` produce the same
    /// node with the same span — so precedence was the only thing left to
    /// decide parentheses by, and it decided badly. `"n = " .. (a + b)` came
    /// back as `"n = " .. a + b`: correct, because `..` is looser than `+`,
    /// and worse, because almost nobody has that rung of the ladder memorised.
    /// A formatter may normalise layout; it should not quietly overrule the
    /// author on what needs spelling out.
    ///
    /// So read it back out of the source, the same trick [`Self::str_lit`]
    /// uses for quote style. `parent_prec` gates it: this is only consulted
    /// for an *operand* of an operator (`0` means a call argument, an index,
    /// a statement's right-hand side — contexts where the surrounding
    /// construct brings its own brackets and the author's parentheses really
    /// are noise).
    ///
    /// Within an operand position the test is exact, not a guess. An operand
    /// has its operator on one side of it: a left operand is followed by
    /// ` op`, a right operand is preceded by `op `. So a `(` immediately
    /// before *and* a `)` immediately after cannot be the enclosing
    /// construct's — only a grouping pair around this operand can be both.
    ///
    /// Returns `false` when there is no source to read (`format_module`),
    /// which is the behaviour that entry point has always had.
    fn was_grouped(&self, parent_prec: u8, span: &Range<usize>) -> bool {
        if parent_prec == 0 {
            return false;
        }
        let before = self
            .source
            .get(..span.start)
            .map(str::trim_end)
            .and_then(|s| s.chars().next_back());
        let after = self
            .source
            .get(span.end..)
            .map(str::trim_start)
            .and_then(|s| s.chars().next());
        before == Some('(') && after == Some(')')
    }

    /// Whether the author wrote this lambda as a `do … end` block after the
    /// closing paren, rather than as a lambda inside the argument list.
    ///
    /// The parser spans a trailing block from its `do` keyword, while a lambda
    /// written in the argument list starts at `fn` or `(`. Once parsed the two
    /// are the same tree, so that first byte is the only surviving record of
    /// which one the author chose — the same trick [`Self::float_lit`] uses to
    /// keep `1f` from being reprinted as `1.0`.
    ///
    /// Preserving the choice matters beyond taste: the trailing form binds to
    /// the callee's last free function-typed parameter, so moving a lambda out
    /// of the parentheses is only a no-op when nothing else could claim that
    /// slot. The formatter has no signatures to check that against.
    ///
    /// With no source to consult (`format_module`) the trailing form is used —
    /// there is no authored spelling to preserve.
    fn is_written_as_do_block(&self, t: &TrailingBlock<'_>) -> bool {
        if self.source.is_empty() {
            return true;
        }
        self.source
            .get(t.span.clone())
            .is_some_and(|s| s.starts_with("do"))
    }

    /// The quote character immediately before `end`, if it is one.
    ///
    /// For `import x from "a/b"` the path has no span of its own — the only
    /// position recorded is the declaration's end, which the parser places
    /// just past the closing quote.
    pub(crate) fn source_quote_ending_at(&self, end: usize) -> Option<char> {
        let c = self.source.get(..end)?.chars().next_back()?;
        (c == '"' || c == '\'').then_some(c)
    }
}

/// A call's arguments split for trailing-block printing: everything that stays
/// inside the parentheses, plus the pieces of the final lambda that moves out
/// after them.
#[derive(Clone, Copy)]
pub(crate) struct TrailingBlock<'t> {
    leading: &'t [CallArg],
    params: &'t [saule_ast::Param],
    return_ty: Option<&'t saule_ast::Type>,
    body: &'t [Spanned<saule_ast::Stmt>],
    /// The lambda's own span, which is what tells the two source forms apart —
    /// see [`Printer::is_written_as_do_block`].
    span: &'t Range<usize>,
}

/// Recognises `f(…, fn(p) … end)` — a call whose last argument is a positional
/// block-bodied lambda — as a *candidate* for printing as `f(…) do (p) … end`.
///
/// Named arguments may precede it (`View(spacing: 10) do … end`); only the
/// final argument's shape matters. Whether the trailing form is actually the
/// one to print is [`Printer::is_written_as_do_block`]'s call.
fn trailing_block_arg(args: &[CallArg]) -> Option<TrailingBlock<'_>> {
    let (last, leading) = args.split_last()?;
    let CallArg::Positional(e) = last else {
        return None;
    };
    let Expr::Lambda {
        params,
        return_ty,
        body: LambdaBody::Block(stmts),
    } = &e.value
    else {
        return None;
    };
    Some(TrailingBlock {
        leading,
        params,
        return_ty: return_ty.as_ref(),
        body: stmts,
        span: &e.span,
    })
}
