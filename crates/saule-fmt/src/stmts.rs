//! Printing statements and blocks, and the `match` arms and patterns
//! they contain.

use saule_ast::{MatchArm, MatchBody, Pattern, Spanned, Stmt, Type};

use super::*;

impl<'a> Printer<'a> {
    /// Print a block body and drain any comments that lie inside it
    /// before returning. `block_end` is the byte offset where the
    /// enclosing construct closes (typically the parent statement's
    /// `span.end`, which sits just past the `end` keyword).
    pub(crate) fn block(&mut self, body: &[Spanned<Stmt>], block_end: usize) {
        self.indent += 1;
        // Anchor `last_pos` at the start of the line containing the
        // first body element. Without this, a comment placed at the
        // top of the block (e.g. immediately under `then` / `do` /
        // `fn(...)`) would see `last_pos` from somewhere above the
        // block header and incorrectly trigger a leading blank line
        // inside `drain_before`.
        if let Some(first) = body.first() {
            self.last_pos = self
                .last_pos
                .max(line_start_in_source(self.source, first.span.start));
        }
        for (i, s) in body.iter().enumerate() {
            let comment_drained = self.drain_before(s.span.start);
            if comment_drained {
                // See `module`: the gap the author left after the comment
                // decides whether it captions the next statement or stands
                // alone as a section header.
                if self.gap_after_comment(s.span.start) {
                    self.blank_line();
                }
            } else if i > 0 {
                let prev_stmt_end = body[i - 1].span.end;
                if self.newlines_in_source(prev_stmt_end, s.span.start) >= 2 {
                    self.blank_line();
                }
            }
            self.stmt(s);
            self.last_pos = self.last_pos.max(s.span.end);
            self.try_trailing(s.span.end);
            self.newline();
        }
        // Comments hanging between the last stmt and the closing keyword.
        self.drain_before(block_end);
        self.indent -= 1;
    }

    pub(crate) fn stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Local {
                name, ty, value, ..
            } => {
                self.write("local ");
                self.write(name);
                if let Some(t) = ty {
                    self.write(": ");
                    self.ty(t);
                }
                if let Some(v) = value {
                    self.write(" = ");
                    self.expr(v, 0);
                }
            }
            Stmt::LocalMulti { names, values } => {
                self.write("local ");
                for (i, (n, _, t)) in names.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.write(n);
                    if let Some(t) = t {
                        self.write(": ");
                        self.ty(t);
                    }
                }
                if !values.is_empty() {
                    self.write(" = ");
                    self.expr_list(values);
                }
            }
            Stmt::Assign { target, value } => {
                self.expr(target, 0);
                self.write(" = ");
                self.expr(value, 0);
            }
            Stmt::CompoundAssign { target, op, value } => {
                self.expr(target, 0);
                self.writef(format_args!(" {}= ", bin_sym(*op)));
                // Precedence 0: the RHS of `op=` is delimited by the end of
                // the statement, so it never needs parenthesising — `x *= a + b`
                // round-trips as written.
                self.expr(value, 0);
            }
            Stmt::AssignMulti { targets, values } => {
                self.expr_list(targets);
                self.write(" = ");
                self.expr_list(values);
            }
            Stmt::Expr(e) => self.expr(e, 0),

            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
                self.write("if ");
                self.expr(cond, 0);
                self.write(" then");
                self.newline();
                // Ceiling for each chunk: start of the next chunk's
                // keyword (approximated by next cond / next stmt).
                let then_ceiling = next_if_chunk_start(elseifs, else_block, s.span.end);
                self.block(then_block, then_ceiling);
                for (i, (c, body)) in elseifs.iter().enumerate() {
                    self.drain_before(c.span.start);
                    self.write("elseif ");
                    self.expr(c, 0);
                    self.write(" then");
                    self.newline();
                    let ceiling = next_if_chunk_start(&elseifs[i + 1..], else_block, s.span.end);
                    self.block(body, ceiling);
                }
                if let Some(eb) = else_block {
                    let else_start = eb.first().map(|st| st.span.start).unwrap_or(s.span.end);
                    self.drain_before(else_start);
                    self.write("else");
                    self.newline();
                    self.block(eb, s.span.end);
                }
                self.write("end");
            }
            Stmt::While { cond, body } => {
                self.write("while ");
                self.expr(cond, 0);
                self.write(" do");
                self.newline();
                self.block(body, s.span.end);
                self.write("end");
            }
            Stmt::Repeat { body, cond } => {
                self.write("repeat");
                self.newline();
                // `until` sits after the body; use cond.span.start as the
                // body's ceiling so trailing comments don't escape past
                // the `until`.
                self.block(body, cond.span.start);
                self.write("until ");
                self.expr(cond, 0);
            }
            Stmt::ForNumeric {
                var,
                var_ty,
                from,
                to,
                step,
                body,
            } => {
                // Lua-style numeric `for`: `for i = from, to[, step] do ... end`.
                self.write("for ");
                self.write(var);
                if let Some(t) = var_ty {
                    self.write(": ");
                    self.ty(t);
                }
                self.write(" = ");
                self.expr(from, 0);
                self.write(", ");
                self.expr(to, 0);
                if let Some(st) = step {
                    self.write(", ");
                    self.expr(st, 0);
                }
                self.write(" do");
                self.newline();
                self.block(body, s.span.end);
                self.write("end");
            }
            Stmt::ForIn { vars, iter, body } => {
                self.write("for ");
                for (i, (n, t)) in vars.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.write(n);
                    if let Some(t) = t {
                        self.write(": ");
                        self.ty(t);
                    }
                }
                self.write(" in ");
                self.expr(iter, 0);
                self.write(" do");
                self.newline();
                self.block(body, s.span.end);
                self.write("end");
            }
            Stmt::Return(vs) => {
                self.write("return");
                if !vs.is_empty() {
                    self.write(" ");
                    self.expr_list(vs);
                }
            }
            Stmt::Throw(e) => {
                self.write("throw ");
                self.expr(e, 0);
            }
            Stmt::Try {
                body,
                catch_var,
                catch_ty,
                catch_body,
            } => {
                self.write("try");
                self.newline();
                let catch_start = catch_body
                    .first()
                    .map(|st| st.span.start)
                    .unwrap_or(s.span.end);
                self.block(body, catch_start);
                self.writef(format_args!("catch {catch_var}: "));
                self.ty(catch_ty);
                self.newline();
                self.block(catch_body, s.span.end);
                self.write("end");
            }
            Stmt::Break => self.write("break"),
            Stmt::Continue => self.write("continue"),
            Stmt::Decl(d) => self.decl(&d.value, d.span.end),
        }
    }

    // ---- declarations ------------------------------------------------------

    pub(crate) fn match_arm(&mut self, a: &MatchArm) {
        self.write("case ");
        self.pattern(&a.pattern);
        if let Some(g) = &a.guard {
            self.write(" when ");
            self.expr(g, 0);
        }
        self.write(" then");
        match &a.body {
            MatchBody::Expr(e) => {
                self.write(" ");
                self.expr(e, 0);
            }
            // Multi-statement arms have no per-arm `end` in Saule — the
            // arm runs until the next `case` or the enclosing `match`'s
            // closing `end`. Indent the body one level past the arm and
            // emit statements without a trailing newline; the caller in
            // `match_expr` handles the separator to the next `case`/`end`.
            MatchBody::Block(stmts) => {
                self.newline();
                self.indent += 1;
                for (i, s) in stmts.iter().enumerate() {
                    self.drain_before(s.span.start);
                    if i > 0 {
                        let prev_end = stmts[i - 1].span.end;
                        if self.newlines_in_source(prev_end, s.span.start) >= 2 {
                            self.blank_line();
                        }
                    }
                    self.stmt(s);
                    self.try_trailing(s.span.end);
                    if i + 1 < stmts.len() {
                        self.newline();
                    }
                }
                self.indent -= 1;
            }
        }
    }

    pub(crate) fn pattern(&mut self, p: &Spanned<Pattern>) {
        match &p.value {
            Pattern::Wildcard => self.write("_"),
            Pattern::Bind(n) => self.write(n),
            Pattern::Nil => self.write("nil"),
            Pattern::Int(n) => self.writef(format_args!("{n}")),
            Pattern::Float(f) => {
                let s = self.float_lit(*f, &p.span);
                self.write(&s);
            }
            Pattern::Bool(b) => self.write(if *b { "true" } else { "false" }),
            Pattern::Str(s) => {
                let lit = self.str_lit(s, &p.span);
                self.write(&lit);
            }
            Pattern::Variant {
                enum_name,
                variant,
                fields,
            } => {
                self.writef(format_args!("{enum_name}.{variant}"));
                if !fields.is_empty() {
                    self.write("(");
                    for (i, f) in fields.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.pattern(f);
                    }
                    self.write(")");
                }
            }
            Pattern::Tuple(ps) => {
                self.write("(");
                for (i, sp) in ps.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.pattern(sp);
                }
                self.write(")");
            }
        }
    }

    // ---- types -------------------------------------------------------------

    pub(crate) fn ty(&mut self, t: &Type) {
        match t {
            Type::Named(n) => self.write(n),
            Type::Nullable(inner) => {
                self.ty(inner);
                self.write("?");
            }
            Type::Table { key, value } => match key {
                Some(k) => {
                    self.write("table<");
                    self.ty(k);
                    self.write(", ");
                    self.ty(value);
                    self.write(">");
                }
                None => {
                    self.write("table<");
                    self.ty(value);
                    self.write(">");
                }
            },
            Type::Tuple(ts) => {
                self.write("(");
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.ty(t);
                }
                self.write(")");
            }
            Type::Function { params, ret } => {
                self.write("fn(");
                for (i, t) in params.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.ty(t);
                }
                self.write(") -> ");
                self.ty(ret);
            }
        }
    }
}
