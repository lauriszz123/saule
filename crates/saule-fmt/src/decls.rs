//! Printing declarations: modules, classes and their members,
//! methods, enum variants, and parameter lists.

use saule_ast::{
    ClassMember, Decl, EnumVariant, ImportNames, Method, MethodSig, Module, Param, Spanned,
};

use super::*;

impl<'a> Printer<'a> {
    pub(crate) fn module(&mut self, m: &Module) {
        for (i, s) in m.stmts.iter().enumerate() {
            let comment_drained = self.drain_before(s.span.start);
            if comment_drained {
                // A comment was just emitted in the gap. Whether it is a
                // caption for the next statement or a standalone section
                // header is the author's call, so honour the gap *they* left
                // between the comment and the statement. This runs for `i == 0`
                // too, which is what keeps a file-header comment from being
                // glued onto the first declaration.
                if self.gap_after_comment(s.span.start) {
                    self.blank_line();
                }
            } else if i > 0 {
                // A blank line either because the author wrote one, or
                // because these two statement kinds always want one.
                let prev_stmt_end = m.stmts[i - 1].span.end;
                if self.newlines_in_source(prev_stmt_end, s.span.start) >= 2
                    || needs_top_separator(&m.stmts[i - 1].value, &s.value)
                {
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

    pub(crate) fn decl(&mut self, d: &Decl, decl_end: usize) {
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
            Decl::Import {
                names,
                path,
                quoted,
            } => {
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
                // Preserve how the author spelled the path — both whether it
                // was quoted at all, and which quote was used.
                if *quoted {
                    let q = self.source_quote_ending_at(decl_end);
                    self.writef(format_args!(" from {}", quote_str_with(path, q)));
                } else {
                    self.writef(format_args!(" from {path}"));
                }
            }
        }
    }

    pub(crate) fn class_body(&mut self, members: &[Spanned<ClassMember>], class_end: usize) {
        self.indent += 1;
        if let Some(first) = members.first() {
            self.last_pos = self
                .last_pos
                .max(line_start_in_source(self.source, first.span.start));
        }
        let mut prev_was_method = false;
        for (i, m) in members.iter().enumerate() {
            let comment_drained = self.drain_before(m.span.start);
            let is_method = matches!(m.value, ClassMember::Method(_));
            if comment_drained {
                // See `module`: the author's gap decides whether the comment
                // captions this member or stands alone above it.
                if self.gap_after_comment(m.span.start) {
                    self.blank_line();
                }
            } else if i > 0 {
                // A blank line either because the author wrote one, or
                // because a method always stands apart from its neighbour.
                let prev_member_end = members[i - 1].span.end;
                if self.newlines_in_source(prev_member_end, m.span.start) >= 2
                    || is_method
                    || prev_was_method
                {
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

    pub(crate) fn class_member(&mut self, m: &ClassMember) {
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

    pub(crate) fn method(&mut self, m: &Method) {
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

    pub(crate) fn method_sig(&mut self, m: &MethodSig) {
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

    pub(crate) fn enum_variant(&mut self, v: &EnumVariant) {
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

    pub(crate) fn type_params(&mut self, tps: &[String]) {
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

    /// Emit a parameter list between an already-written `(` and the `)` the
    /// caller writes next. Wraps like [`Printer::call_args`] when the
    /// signature would run past the width target.
    ///
    /// The budget reserves one column for `)` but not for a trailing
    /// `-> ReturnType`, so a signature with a long return type can still edge
    /// slightly past the target — it is a soft limit, and breaking a
    /// just-barely-long signature reads worse than the overhang.
    ///
    /// As in [`Printer::call_args`], the comma separates rather than
    /// terminates: `parse_param_list_inner` demands a parameter after every
    /// comma, so a trailing one would not parse back.
    pub(crate) fn params(&mut self, ps: &[Param]) {
        if ps.is_empty() {
            return;
        }
        let start_col = self.current_column();
        let inline = self.render_params_inline(ps);
        if self.force_inline || start_col + inline.len() < self.max_width() {
            self.write(&inline);
            return;
        }

        self.indent += 1;
        for (i, p) in ps.iter().enumerate() {
            self.newline();
            self.write_param(p);
            if i + 1 < ps.len() {
                self.write(",");
            }
        }
        self.indent -= 1;
        self.newline();
    }

    pub(crate) fn render_params_inline(&self, ps: &[Param]) -> String {
        let mut sub = self.sub_printer();
        for (i, p) in ps.iter().enumerate() {
            if i > 0 {
                sub.write(", ");
            }
            sub.write_param(p);
        }
        sub.out
    }

    /// Emit a single parameter — shared by the inline and multi-line layouts.
    pub(crate) fn write_param(&mut self, p: &Param) {
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

    // ---- expressions -------------------------------------------------------
}
