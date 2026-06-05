//! Saule source pretty-printer.
//!
//! Walks a parsed [`saule_ast::Module`] and renders it back to canonical
//! Saule source: 2-space indent, one statement per line, blank line between
//! top-level declarations.
//!
//! ## Comment preservation
//!
//! [`format_module`] discards comments — the AST never sees them. To round
//! trip comments, use [`format_module_with_comments`] together with the
//! lexer's `tokenize_with_trivia` entry point: extract every
//! [`saule_lexer::Token::LineComment`] / `BlockComment` into a [`Comment`]
//! and pass it in.
//!
//! Interleaving is best-effort but covers the common shapes:
//!
//! * Comments before a statement / declaration are emitted on their own
//!   line at the surrounding indent.
//! * A comment that sits on the same source line as the statement it
//!   trails is re-emitted as a same-line trailing comment.
//! * Comments at the tail of a block (just before the closing `end`) are
//!   drained at the right indent so they don't leak past the block.
//! * Blank lines between source comments are preserved when ≥ 2 newlines
//!   separated them in the original source.

use std::{collections::VecDeque, fmt::Write, ops::Range};

use saule_ast::{
    BinOp, CallArg, ClassMember, Decl, EnumVariant, Expr, ImportNames, LambdaBody, MatchArm,
    MatchBody, Method, MethodSig, Module, Param, Pattern, PipeStage, Spanned, Stmt, TableEntry,
    Type, UnaryOp,
};

/// A single source comment extracted from the lexer's trivia stream.
/// `text` is the verbatim payload between the comment delimiters (no
/// `--` / `--[[` / `]]`), matching what `tokenize_with_trivia` emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub span: Range<usize>,
    pub kind: CommentKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// `-- text` to end of line.
    Line,
    /// `--[[ text ]]` (may span multiple source lines).
    Block,
}

/// Render a parsed module back to source, dropping any comments. Use
/// [`format_module_with_comments`] to preserve them. The result always
/// ends with exactly one trailing newline (or is empty for an empty
/// module).
pub fn format_module(module: &Module) -> String {
    let mut p = Printer::new("", &[]);
    p.module(module);
    p.finish()
}

/// Like [`format_module`] but threads `comments` (sorted or not, by span
/// start) back into the output. `source` is the original text, used to
/// tell same-line trailing comments from leading ones and to preserve
/// blank-line gaps.
pub fn format_module_with_comments(module: &Module, source: &str, comments: &[Comment]) -> String {
    let mut p = Printer::new(source, comments);
    p.module(module);
    p.finish()
}

struct Printer<'a> {
    out: String,
    indent: usize,
    /// Set right after a newline so the next `write_str` knows to prepend
    /// the current indentation. Avoids trailing whitespace on blank lines.
    needs_indent: bool,
    /// Original source text. Only consulted for newline-counting between
    /// byte positions when interleaving comments; empty when comments are
    /// disabled.
    source: &'a str,
    /// Pending comments, ordered by `span.start`. Drained as the printer
    /// reaches the corresponding source positions.
    comments: VecDeque<&'a Comment>,
    /// Highest source offset we've "consumed" so far — either the end of
    /// the last comment we drained, or 0. Used for blank-line preservation
    /// between consecutive comments.
    last_pos: usize,
}

const INDENT: &str = "  ";

/// Soft target for one rendered line. The `Expr::Pipe` layout uses this to
/// decide whether a `when(...):a():b()` chain fits inline or should be
/// broken across multiple lines. Anything past this threshold flips to
/// the column-aligned multi-line shape.
const MAX_LINE_WIDTH: usize = 100;

impl<'a> Printer<'a> {
    fn new(source: &'a str, comments: &'a [Comment]) -> Self {
        let mut queue: VecDeque<&'a Comment> = comments.iter().collect();
        // Tolerate unsorted inputs.
        queue.make_contiguous().sort_by_key(|c| c.span.start);
        Self {
            out: String::new(),
            indent: 0,
            needs_indent: false,
            source,
            comments: queue,
            last_pos: 0,
        }
    }

    fn finish(mut self) -> String {
        // Guarantee exactly one trailing newline, even for an empty
        // module — every formatted file ends with `\n`.
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    fn write(&mut self, s: &str) {
        if self.needs_indent {
            for _ in 0..self.indent {
                self.out.push_str(INDENT);
            }
            self.needs_indent = false;
        }
        self.out.push_str(s);
    }

    fn writef(&mut self, args: std::fmt::Arguments<'_>) {
        if self.needs_indent {
            for _ in 0..self.indent {
                self.out.push_str(INDENT);
            }
            self.needs_indent = false;
        }
        let _ = self.out.write_fmt(args);
    }

    fn newline(&mut self) {
        self.out.push('\n');
        self.needs_indent = true;
    }

    fn blank_line(&mut self) {
        // Don't emit duplicate blank lines or a blank line at the start.
        if self.out.is_empty() {
            return;
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        if !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
        self.needs_indent = true;
    }

    // ---- comment interleaving ---------------------------------------------

    /// Emit every queued comment whose start lies strictly before `pos`,
    /// one per line at the current indent. Preserves blank-line gaps
    /// between consecutive source comments (≥ 2 newlines in the original
    /// source → blank line in the output).
    fn drain_before(&mut self, pos: usize) {
        while let Some(c) = self.comments.front() {
            if c.span.start >= pos {
                break;
            }
            let c = self.comments.pop_front().unwrap();
            // If the printer's current line already has content, this
            // comment can't be a "leading" one — push it to its own line
            // first. (Happens when called mid-construct; rare but cheap.)
            if !self.out.is_empty() && !self.out.ends_with('\n') {
                self.newline();
            }
            // Blank-line preservation between comments / from start of file.
            if self.newlines_in_source(self.last_pos, c.span.start) >= 2 {
                self.blank_line();
            }
            self.write_comment(c);
            self.newline();
            self.last_pos = self.last_pos.max(c.span.end);
        }
    }

    /// If the next pending comment starts on the same source line as
    /// `after_pos`, emit it as a same-line trailing comment (preceded by
    /// two spaces) and consume it. Returns `true` if a comment was
    /// emitted, so callers can skip their own trailing newline logic if
    /// they want.
    fn try_trailing(&mut self, after_pos: usize) -> bool {
        let Some(c) = self.comments.front() else {
            return false;
        };
        if c.span.start < after_pos {
            // Pathological: drain_before should have handled it. Be safe.
            return false;
        }
        if self.newlines_in_source(after_pos, c.span.start) > 0 {
            return false;
        }
        let c = self.comments.pop_front().unwrap();
        self.out.push_str("  ");
        self.write_comment(c);
        self.last_pos = self.last_pos.max(c.span.end);
        true
    }

    fn write_comment(&mut self, c: &Comment) {
        // `write_str` would apply indentation; we want indent only on the
        // first line. Manage it manually.
        if self.needs_indent {
            for _ in 0..self.indent {
                self.out.push_str(INDENT);
            }
            self.needs_indent = false;
        }
        match c.kind {
            CommentKind::Line => {
                self.out.push_str("--");
                self.out.push_str(&c.text);
            }
            CommentKind::Block => {
                self.out.push_str("--[[");
                self.out.push_str(&c.text);
                self.out.push_str("]]");
            }
        }
    }

    /// Count `\n` bytes in `source[from..to]`. Returns 0 if either bound
    /// is out of range or if `from > to`. Used to distinguish same-line
    /// trailing comments from leading ones, and to preserve blank lines.
    fn newlines_in_source(&self, from: usize, to: usize) -> usize {
        if from > to || to > self.source.len() {
            return 0;
        }
        self.source.as_bytes()[from..to]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
    }
    // ---- top-level ---------------------------------------------------------

    fn module(&mut self, m: &Module) {
        for (i, s) in m.stmts.iter().enumerate() {
            self.drain_before(s.span.start);
            if i > 0 {
                let prev_stmt_end = m.stmts[i - 1].span.end;
                let comment_drained = self.last_pos > prev_stmt_end;
                if comment_drained {
                    // A comment was just emitted in the gap. Treat it as
                    // attached to the upcoming stmt: no blank line between
                    // the comment and the stmt, regardless of source.
                } else if self.newlines_in_source(prev_stmt_end, s.span.start)
                    >= 2
                {
                    self.blank_line();
                } else if needs_top_separator(&m.stmts[i - 1].value, &s.value) {
                    self.blank_line();
                }
            }
            self.stmt(s);
            self.last_pos = self.last_pos.max(s.span.end);
            self.try_trailing(s.span.end);
            self.newline();
        }
        // Trailing comments at end-of-file (after the last statement).
        self.drain_before(usize::MAX);
    }

    // ---- statements --------------------------------------------------------

    /// Print a block body and drain any comments that lie inside it
    /// before returning. `block_end` is the byte offset where the
    /// enclosing construct closes (typically the parent statement's
    /// `span.end`, which sits just past the `end` keyword).
    fn block(&mut self, body: &[Spanned<Stmt>], block_end: usize) {
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
            self.drain_before(s.span.start);
            if i > 0 {
                let prev_stmt_end = body[i - 1].span.end;
                let comment_drained = self.last_pos > prev_stmt_end;
                if comment_drained {
                    // Comment attaches to the next stmt; no blank between
                    // the comment and the stmt.
                } else if self.newlines_in_source(prev_stmt_end, s.span.start)
                    >= 2
                {
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

    fn stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Local { name, ty, value, .. } => {
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

    fn decl(&mut self, d: &Decl, decl_end: usize) {
        match d {
            Decl::Function {
                exported,
                is_local,
                name,
                type_params,
                params,
                return_ty,
                body,
            } => {
                if *exported {
                    self.write("export ");
                } else if *is_local {
                    self.write("local ");
                }
                self.write("fn ");
                self.write(name);
                self.type_params(type_params);
                self.write("(");
                self.params(params);
                self.write(")");
                if let Some(rt) = return_ty {
                    self.write(" -> ");
                    self.ty(rt);
                }
                self.newline();
                self.block(body, decl_end);
                self.write("end");
            }
            Decl::Class {
                exported,
                name,
                extends,
                implements,
                members,
            } => {
                if *exported {
                    self.write("export ");
                }
                self.write("class ");
                self.write(name);
                if let Some(p) = extends {
                    self.write(" extends ");
                    self.write(p);
                }
                if !implements.is_empty() {
                    self.write(" implements ");
                    for (i, n) in implements.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.write(n);
                    }
                }
                self.newline();
                self.class_body(members, decl_end);
                self.write("end");
            }
            Decl::Interface {
                exported,
                name,
                extends,
                methods,
            } => {
                if *exported {
                    self.write("export ");
                }
                self.write("interface ");
                self.write(name);
                if !extends.is_empty() {
                    self.write(" extends ");
                    for (i, n) in extends.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.write(n);
                    }
                }
                self.newline();
                self.indent += 1;
                for m in methods {
                    self.drain_before(m.span.start);
                    self.method_sig(m);
                    self.try_trailing(m.span.end);
                    self.newline();
                }
                self.drain_before(decl_end);
                self.indent -= 1;
                self.write("end");
            }
            Decl::Enum {
                exported,
                name,
                variants,
                methods,
            } => {
                if *exported {
                    self.write("export ");
                }
                self.write("enum ");
                self.write(name);
                self.newline();
                self.indent += 1;
                for (i, v) in variants.iter().enumerate() {
                    self.drain_before(v.span.start);
                    self.enum_variant(&v.value);
                    if i + 1 < variants.len() {
                        self.write(",");
                    }
                    self.try_trailing(v.span.end);
                    self.newline();
                }
                if !methods.is_empty() {
                    let first_method_start = methods[0].span.start;
                    self.drain_before(first_method_start);
                    self.blank_line();
                    for (i, m) in methods.iter().enumerate() {
                        if i > 0 {
                            self.drain_before(m.span.start);
                            self.blank_line();
                        }
                        self.method(m);
                        self.try_trailing(m.span.end);
                        self.newline();
                    }
                }
                self.drain_before(decl_end);
                self.indent -= 1;
                self.write("end");
            }
            Decl::Import { names, path } => {
                self.write("import ");
                match names {
                    ImportNames::All => self.write("*"),
                    ImportNames::List(ns) => {
                        for (i, (orig, alias)) in ns.iter().enumerate() {
                            if i > 0 {
                                self.write(", ");
                            }
                            self.write(orig);
                            if let Some(a) = alias {
                                self.writef(format_args!(" as {a}"));
                            }
                        }
                    }
                }
                self.writef(format_args!(" from {}", quote_str(path)));
            }
        }
    }

    fn class_body(&mut self, members: &[Spanned<ClassMember>], class_end: usize) {
        self.indent += 1;
        if let Some(first) = members.first() {
            self.last_pos = self
                .last_pos
                .max(line_start_in_source(self.source, first.span.start));
        }
        let mut prev_was_method = false;
        for (i, m) in members.iter().enumerate() {
            self.drain_before(m.span.start);
            let is_method = matches!(m.value, ClassMember::Method(_));
            if i > 0 {
                let prev_member_end = members[i - 1].span.end;
                let comment_drained = self.last_pos > prev_member_end;
                if comment_drained {
                    // Comment attaches to the next member.
                } else if self.newlines_in_source(prev_member_end, m.span.start)
                    >= 2
                {
                    self.blank_line();
                } else if is_method || prev_was_method {
                    self.blank_line();
                }
            }
            self.class_member(&m.value);
            self.last_pos = self.last_pos.max(m.span.end);
            self.try_trailing(m.span.end);
            self.newline();
            prev_was_method = is_method;
        }
        self.drain_before(class_end);
        self.indent -= 1;
    }

    fn class_member(&mut self, m: &ClassMember) {
        match m {
            ClassMember::Field {
                is_static,
                is_private,
                name,
                ty,
                default,
            } => {
                if *is_static {
                    self.write("static ");
                }
                if *is_private {
                    self.write("local ");
                }
                self.write(name);
                self.write(": ");
                self.ty(ty);
                if let Some(d) = default {
                    self.write(" = ");
                    self.expr(d, 0);
                }
            }
            ClassMember::Method(m) => self.method(m),
        }
    }

    fn method(&mut self, m: &Method) {
        if m.is_static {
            self.write("static ");
        }
        if m.is_private {
            self.write("local ");
        }
        self.write("fn ");
        self.write(&m.name);
        self.type_params(&m.type_params);
        self.write("(");
        self.params(&m.params);
        self.write(")");
        if let Some(rt) = &m.return_ty {
            self.write(" -> ");
            self.ty(rt);
        }
        self.newline();
        self.block(&m.body, m.span.end);
        self.write("end");
    }

    fn method_sig(&mut self, m: &MethodSig) {
        self.write("fn ");
        self.write(&m.name);
        self.write("(");
        self.params(&m.params);
        self.write(")");
        if let Some(rt) = &m.return_ty {
            self.write(" -> ");
            self.ty(rt);
        }
    }

    fn enum_variant(&mut self, v: &EnumVariant) {
        match v {
            EnumVariant::Bare(n) => self.write(n),
            EnumVariant::Valued(n, e) => {
                self.write(n);
                self.write(" = ");
                self.expr(e, 0);
            }
            EnumVariant::Tuple { name, fields } => {
                self.write(name);
                self.write("(");
                self.params(fields);
                self.write(")");
            }
        }
    }

    fn type_params(&mut self, tps: &[String]) {
        if tps.is_empty() {
            return;
        }
        self.write("<");
        for (i, t) in tps.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.write(t);
        }
        self.write(">");
    }

    fn params(&mut self, ps: &[Param]) {
        for (i, p) in ps.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            if p.variadic {
                self.write("...");
            }
            self.write(&p.name);
            self.write(": ");
            self.ty(&p.ty);
            if let Some(d) = &p.default {
                self.write(" = ");
                self.expr(d, 0);
            }
        }
    }

    // ---- expressions -------------------------------------------------------

    fn expr_list(&mut self, es: &[Spanned<Expr>]) {
        for (i, e) in es.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.expr(e, 0);
        }
    }

    /// Print an expression. `parent_prec` is the binding strength of the
    /// surrounding context; if our own precedence is lower we wrap in
    /// parens to preserve grouping.
    fn expr(&mut self, e: &Spanned<Expr>, parent_prec: u8) {
        let outer_end = e.span.end;
        match &e.value {
            Expr::Int(n) => self.writef(format_args!("{n}")),
            Expr::Float(f) => self.write(&format_float(*f)),
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
            Expr::MethodCall { obj, method, args } => {
                self.expr(obj, MAX_PREC);
                self.write(":");
                self.write(method);
                self.write("(");
                self.call_args(args);
                self.write(")");
            }
            Expr::ForceUnwrap(inner) => {
                self.expr(inner, MAX_PREC);
                self.write("!");
            }

            Expr::Table(entries) => {
                if entries.is_empty() {
                    self.write("{}");
                } else {
                    // Mirrors the `when(...)` layout policy: inline by default,
                    // multi-line when the inline form overflows
                    // [`MAX_LINE_WIDTH`] OR the user already broke an entry
                    // onto its own line in the source.
                    let start_col = self.current_column();
                    let inline = self.render_table_inline(entries);
                    let force_ml = self.table_has_source_break(entries);
                    let too_wide = start_col + inline.len() > MAX_LINE_WIDTH;
                    if !force_ml && !too_wide {
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
            //   * Inline by default when the whole chain fits within
            //     [`MAX_LINE_WIDTH`] from the current column. Reads as
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
                let too_wide = when_col + inline.len() > MAX_LINE_WIDTH;
                if !force_ml && !too_wide {
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

    /// Column (0-based) of the *next* character to be emitted.
    ///
    /// Used by [`Expr::Pipe`] to remember where the `w` of `when` lands
    /// so subsequent `:stage()` lines can align under it. Accounts for a
    /// pending indent that hasn't been flushed yet.
    fn current_column(&self) -> usize {
        if self.needs_indent {
            return self.indent * INDENT.len();
        }
        match self.out.rfind('\n') {
            Some(p) => self.out.len() - p - 1,
            None => self.out.len(),
        }
    }

    /// `true` when the user broke any `:stage()` onto its own line in the
    /// original source. We honour that — once it's multi-line in the
    /// source it stays multi-line in the output, even if the chain is
    /// short enough to fit inline.
    fn pipe_has_source_break(&self, source: &Spanned<Expr>, stages: &[PipeStage]) -> bool {
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

    fn source_range_has_newline(&self, range: Range<usize>) -> bool {
        self.source
            .get(range)
            .map(|s| s.contains('\n'))
            .unwrap_or(false)
    }

    /// Render `when(source):stage1(args):stage2(args)…` into a string
    /// without touching `self.out`. The result is the inline form; the
    /// caller compares its length against the available room to decide
    /// whether to commit it or fall back to the multi-line layout.
    fn render_pipe_inline(&self, source: &Spanned<Expr>, stages: &[PipeStage]) -> String {
        let mut sub = Printer {
            out: String::new(),
            indent: self.indent,
            needs_indent: false,
            // Comments inside the chain are ignored for the size estimate;
            // they only ever fire in the real `self` printer.
            source: self.source,
            comments: VecDeque::new(),
            last_pos: self.last_pos,
        };
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
    /// sub-printer. Used to decide whether the inline form fits within
    /// [`MAX_LINE_WIDTH`] before committing to a layout.
    fn render_table_inline(&self, entries: &[TableEntry]) -> String {
        let mut sub = Printer {
            out: String::new(),
            indent: self.indent,
            needs_indent: false,
            source: self.source,
            comments: VecDeque::new(),
            last_pos: self.last_pos,
        };
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
    fn table_has_source_break(&self, entries: &[TableEntry]) -> bool {
        if self.source.is_empty() || entries.len() < 2 {
            return false;
        }
        entries
            .windows(2)
            .any(|pair| self.source_range_has_newline(entry_end(&pair[0])..entry_start(&pair[1])))
    }

    /// Emit a single table entry — shared by inline and multi-line layouts.
    fn write_table_entry(&mut self, ent: &TableEntry) {
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

    fn call_args(&mut self, args: &[CallArg]) {
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            match a {
                CallArg::Positional(e) => self.expr(e, 0),
                CallArg::Named { name, value } => {
                    self.writef(format_args!("{name}: "));
                    self.expr(value, 0);
                }
            }
        }
    }

    fn match_arm(&mut self, a: &MatchArm) {
        self.write("case ");
        self.pattern(&a.pattern.value);
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

    fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Wildcard => self.write("_"),
            Pattern::Bind(n) => self.write(n),
            Pattern::Nil => self.write("nil"),
            Pattern::Int(n) => self.writef(format_args!("{n}")),
            Pattern::Float(f) => self.write(&format_float(*f)),
            Pattern::Bool(b) => self.write(if *b { "true" } else { "false" }),
            Pattern::Str(s) => self.write(&quote_str(s)),
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
                        self.pattern(&f.value);
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
                    self.pattern(&sp.value);
                }
                self.write(")");
            }
        }
    }

    // ---- types -------------------------------------------------------------

    fn ty(&mut self, t: &Type) {
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

// ---- precedence / formatting helpers ---------------------------------------

/// Higher than every `bin_prec`, used as the lower bound for operands of
/// unary / postfix expressions so they always parenthesize inner binaries.
const MAX_PREC: u8 = 100;

/// (precedence, right_associative) for each binary operator, mirroring the
/// parser's Pratt table closely enough that re-parsing produces the same
/// tree.
fn bin_prec(op: BinOp) -> (u8, bool) {
    match op {
        BinOp::Or | BinOp::Coalesce => (1, false),
        BinOp::And => (2, false),
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => (3, false),
        BinOp::Concat => (4, true),
        BinOp::Add | BinOp::Sub => (5, false),
        BinOp::Mul | BinOp::Div | BinOp::Mod => (6, false),
    }
}

fn bin_sym(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::LtEq => "<=",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Concat => "..",
        BinOp::Coalesce => "??",
    }
}

/// Render an `f64` so it always reads back as a float (i.e. `1.0` rather
/// than `1`) and round-trips through `parse::<f64>()` losslessly for
/// finite values.
fn format_float(f: f64) -> String {
    if !f.is_finite() {
        // Saule has no syntax for these; keep something readable.
        return format!("{f}");
    }
    let s = format!("{f}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

fn quote_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether a single-param lambda came from the `name => expr` shortcut
/// (no type annotation, no return type, default-`any`). Matches the
/// parser's reconstruction so re-parsing produces the same AST.
fn is_bare_arrow_param(params: &[Param], return_ty: &Option<Type>) -> bool {
    if return_ty.is_some() || params.len() != 1 {
        return false;
    }
    let p = &params[0];
    !p.variadic && p.default.is_none() && matches!(&p.ty, Type::Named(n) if n == "any")
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Byte offset where a table entry starts in the source — used by the
/// formatter to detect a user-introduced line break between two entries
/// so the multi-line layout sticks.
fn entry_start(entry: &TableEntry) -> usize {
    match entry {
        TableEntry::Positional(e) => e.span.start,
        TableEntry::Field { key, .. } => key.span.start,
    }
}

/// Byte offset where a table entry ends in the source.
fn entry_end(entry: &TableEntry) -> usize {
    match entry {
        TableEntry::Positional(e) => e.span.end,
        TableEntry::Field { value, .. } => value.span.end,
    }
}

/// Whether two adjacent top-level statements should be separated by a
/// blank line. Declarations get breathing room; tight runs of locals or
/// expression statements stay compact. Consecutive `import` statements
/// are an exception — they read as a single block and stay packed.
fn needs_top_separator(prev: &Stmt, next: &Stmt) -> bool {
    let p_is_import = matches!(prev, Stmt::Decl(d) if matches!(d.value, Decl::Import { .. }));
    let n_is_import = matches!(next, Stmt::Decl(d) if matches!(d.value, Decl::Import { .. }));
    if p_is_import && n_is_import {
        return false;
    }
    let p_is_decl = matches!(prev, Stmt::Decl(_));
    let n_is_decl = matches!(next, Stmt::Decl(_));
    p_is_decl || n_is_decl
}

/// Byte offset of the first character on the line that contains `pos`.
/// Walks backwards in `source` to find the previous `\n`; returns
/// `pos` itself when out of range. Used at block entry to anchor
/// `last_pos` so a comment placed right under a header doesn't get
/// charged for the newlines above the header.
fn line_start_in_source(source: &str, pos: usize) -> usize {
    if pos > source.len() {
        return source.len();
    }
    source[..pos].rfind('\n').map(|n| n + 1).unwrap_or(0)
}

/// The byte offset where the next chunk of an `if … elseif … else … end`
/// starts. Used as the body-block ceiling when draining comments so they
/// don't escape past the `elseif` / `else` keyword.
fn next_if_chunk_start(
    remaining_elseifs: &[(Spanned<Expr>, Vec<Spanned<Stmt>>)],
    else_block: &Option<Vec<Spanned<Stmt>>>,
    fallback: usize,
) -> usize {
    if let Some((cond, _)) = remaining_elseifs.first() {
        return cond.span.start;
    }
    if let Some(eb) = else_block {
        if let Some(first) = eb.first() {
            return first.span.start;
        }
    }
    fallback
}
