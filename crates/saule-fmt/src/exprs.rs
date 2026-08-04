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
            Expr::Str(s) => self.write(&quote_str(s)),
            Expr::Nil => self.write("nil"),
            Expr::Ident(n) => self.write(n),
            Expr::Self_ => self.write("self"),

            Expr::Unary { op, rhs } => {
                let (sym, space) = match op {
                    UnaryOp::Neg => ("-", false),
                    UnaryOp::Not => ("not", true),
                    UnaryOp::Len => ("#", false),
                };
                self.write(sym);
                if space {
                    self.write(" ");
                }
                // Unary binds tighter than any binary, so wrap operands of
                // strictly lower precedence than `MAX_PREC`.
                self.expr(rhs, MAX_PREC);
            }
            Expr::Binary { op, lhs, rhs } => {
                let (p, right_assoc) = bin_prec(*op);
                if p < parent_prec {
                    self.write("(");
                }
                let left_min = if right_assoc { p + 1 } else { p };
                let right_min = if right_assoc { p } else { p + 1 };
                self.expr(lhs, left_min);
                self.writef(format_args!(" {} ", bin_sym(*op)));
                self.expr(rhs, right_min);
                if p < parent_prec {
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
                self.expr(callee, MAX_PREC);
                self.write("(");
                self.call_args(args);
                self.write(")");
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
                let parens = CAST_PREC < parent_prec;
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
                        self.write(&quote_str(s));
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
}
