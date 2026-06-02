//! Cursor-position resolver — walks the AST and identifies what
//! semantic symbol the byte offset is on. Used by goto-definition.

use std::ops::Range;

use saule_ast::{
    CallArg, ClassMember, Decl, EnumVariant, Expr, LambdaBody, MatchBody, Method, Module, Param,
    Pattern, Spanned, Stmt, TableEntry, Type,
};
use saule_semantic::{lookup_field_type, lookup_method, with_classes, with_enums, with_interfaces};

use super::util::{
    contains, inferred_type_of, locate_string_literal, locate_word_in, member_name_span,
    named_type, strip_nullable, LocalBind,
};
use super::{Resolved, Symbol};

pub(super) fn run(module: &Module, source: &str, offset: usize) -> Option<Resolved> {
    let mut cx = ResolveCx {
        offset,
        source,
        enclosing_class: None,
        locals: Vec::new(),
        best: None,
    };
    cx.visit_module(module);
    cx.best
}

struct ResolveCx<'a> {
    offset: usize,
    source: &'a str,
    enclosing_class: Option<String>,
    locals: Vec<LocalBind>,
    best: Option<Resolved>,
}

impl<'a> ResolveCx<'a> {
    fn record(&mut self, span: Range<usize>, symbol: Symbol) {
        if !contains(&span, self.offset) {
            return;
        }
        let new_w = span.end.saturating_sub(span.start);
        if let Some(prev) = &self.best {
            let cur_w = prev.span.end.saturating_sub(prev.span.start);
            if new_w >= cur_w {
                return;
            }
        }
        self.best = Some(Resolved { symbol, span });
    }

    fn lookup_local(&self, name: &str) -> Option<&LocalBind> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

    fn enter_function(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let saved = std::mem::take(&mut self.locals);
        for p in params {
            let def_span =
                locate_word_in(self.source, &p.span, &p.name).unwrap_or(p.span.clone());
            self.locals.push(LocalBind {
                name: p.name.clone(),
                def_span,
                ty: p.ty.clone(),
            });
        }
        body(self);
        self.locals = saved;
    }

    /// Best-effort receiver type resolution, mirroring the hover
    /// receiver_class logic so member/method goto navigates to the
    /// right class.
    fn receiver_class(&self, obj: &Expr) -> Option<String> {
        match obj {
            Expr::Self_ => self.enclosing_class.clone(),
            Expr::Ident(name) => {
                if let Some(local) = self.lookup_local(name) {
                    return named_type(&local.ty);
                }
                if with_classes(|r| r.contains_key(name))
                    || with_enums(|r| r.contains_key(name))
                    || saule_typeck::sigs::is_module(name)
                    || saule_typeck::sigs::is_value_type(name)
                {
                    Some(name.clone())
                } else {
                    None
                }
            }
            Expr::Member { obj: inner, name } => {
                let inner_class = self.receiver_class(&inner.value)?;
                let ty = lookup_field_type(&inner_class, name)?;
                named_type(&ty)
            }
            Expr::Call { callee, .. } => {
                if let Expr::Ident(n) = &callee.value
                    && with_classes(|r| r.contains_key(n))
                {
                    return Some(n.clone());
                }
                None
            }
            Expr::MethodCall { obj: inner, method, .. } => {
                let inner_class = self.receiver_class(&inner.value)?;
                let sig = lookup_method(&inner_class, method)?;
                named_type(sig.return_ty.as_ref()?)
            }
            _ => None,
        }
    }

    fn infer_local_ty(&self, init: &Expr) -> Type {
        match init {
            Expr::Self_ => self
                .enclosing_class
                .as_ref()
                .map(|c| Type::Named(c.clone()))
                .unwrap_or_else(|| Type::Named("any".into())),
            Expr::Ident(n) => self
                .lookup_local(n)
                .map(|l| l.ty.clone())
                .unwrap_or_else(|| Type::Named("any".into())),
            Expr::Call { callee, .. } => {
                if let Expr::Ident(n) = &callee.value
                    && with_classes(|r| r.contains_key(n))
                {
                    return Type::Named(n.clone());
                }
                Type::Named("any".into())
            }
            Expr::MethodCall { obj, method, .. } => {
                if let Some(class) = self.receiver_class(&obj.value)
                    && let Some(sig) = lookup_method(&class, method)
                    && let Some(rt) = sig.return_ty
                {
                    return rt;
                }
                Type::Named("any".into())
            }
            _ => Type::Named("any".into()),
        }
    }

    fn visit_module(&mut self, m: &Module) {
        for s in &m.stmts {
            self.visit_stmt(s);
        }
    }

    fn visit_stmt(&mut self, s: &Spanned<Stmt>) {
        if !contains(&s.span, self.offset) && !matches!(&s.value, Stmt::Decl(_)) {
            // Locals declared in this stmt still need to be pushed
            // even when the cursor isn't in it, so following stmts
            // resolve correctly. Drop only when we're sure we're past.
            if s.span.end < self.offset {
                self.push_locals_from_stmt(s);
            }
            return;
        }
        match &s.value {
            Stmt::Decl(d) => self.visit_decl(d),
            Stmt::Local { name, ty, value } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                if let Some(span) = locate_word_in(self.source, &s.span, name) {
                    self.record(span.clone(), Symbol::Local {
                        name: name.clone(),
                        def_span: span,
                    });
                }
                self.push_local_binding(name, ty.clone(), value.as_ref().map(|v| &v.value), &s.span);
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.visit_expr(v);
                }
                for (i, (name, ty)) in names.iter().enumerate() {
                    if let Some(span) = locate_word_in(self.source, &s.span, name) {
                        self.record(span.clone(), Symbol::Local {
                            name: name.clone(),
                            def_span: span,
                        });
                    }
                    let init = values.get(i).map(|v| &v.value);
                    self.push_local_binding(name, ty.clone(), init, &s.span);
                }
            }
            Stmt::Assign { target, value } => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            Stmt::AssignMulti { targets, values } => {
                for t in targets {
                    self.visit_expr(t);
                }
                for v in values {
                    self.visit_expr(v);
                }
            }
            Stmt::Expr(e) => self.visit_expr(e),
            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
                self.visit_expr(cond);
                self.with_block(|this| {
                    for s in then_block {
                        this.visit_stmt(s);
                    }
                });
                for (c, b) in elseifs {
                    self.visit_expr(c);
                    self.with_block(|this| {
                        for s in b {
                            this.visit_stmt(s);
                        }
                    });
                }
                if let Some(eb) = else_block {
                    self.with_block(|this| {
                        for s in eb {
                            this.visit_stmt(s);
                        }
                    });
                }
            }
            Stmt::While { cond, body } => {
                self.visit_expr(cond);
                self.with_block(|this| {
                    for s in body {
                        this.visit_stmt(s);
                    }
                });
            }
            Stmt::Repeat { body, cond } => {
                self.with_block(|this| {
                    for s in body {
                        this.visit_stmt(s);
                    }
                });
                self.visit_expr(cond);
            }
            Stmt::ForNumeric {
                var,
                var_ty,
                from,
                to,
                step,
                body,
            } => {
                self.visit_expr(from);
                self.visit_expr(to);
                if let Some(st) = step {
                    self.visit_expr(st);
                }
                let span = locate_word_in(self.source, &s.span, var)
                    .unwrap_or_else(|| s.span.clone());
                self.record(span.clone(), Symbol::Local {
                    name: var.clone(),
                    def_span: span.clone(),
                });
                let saved = self.locals.len();
                self.locals.push(LocalBind {
                    name: var.clone(),
                    def_span: span,
                    ty: var_ty.clone().unwrap_or_else(|| Type::Named("integer".into())),
                });
                for s in body {
                    self.visit_stmt(s);
                }
                self.locals.truncate(saved);
            }
            Stmt::ForIn { vars, iter, body } => {
                self.visit_expr(iter);
                let saved = self.locals.len();
                for (name, ty) in vars {
                    let span = locate_word_in(self.source, &s.span, name)
                        .unwrap_or_else(|| s.span.clone());
                    self.record(span.clone(), Symbol::Local {
                        name: name.clone(),
                        def_span: span.clone(),
                    });
                    self.locals.push(LocalBind {
                        name: name.clone(),
                        def_span: span,
                        ty: ty.clone().unwrap_or_else(|| Type::Named("any".into())),
                    });
                }
                for s in body {
                    self.visit_stmt(s);
                }
                self.locals.truncate(saved);
            }
            Stmt::Return(es) => {
                for e in es {
                    self.visit_expr(e);
                }
            }
            Stmt::Throw(e) => self.visit_expr(e),
            Stmt::Try {
                body,
                catch_var,
                catch_ty,
                catch_body,
            } => {
                self.with_block(|this| {
                    for s in body {
                        this.visit_stmt(s);
                    }
                });
                let span = locate_word_in(self.source, &s.span, catch_var)
                    .unwrap_or_else(|| s.span.clone());
                self.record(span.clone(), Symbol::Local {
                    name: catch_var.clone(),
                    def_span: span.clone(),
                });
                let saved = self.locals.len();
                self.locals.push(LocalBind {
                    name: catch_var.clone(),
                    def_span: span,
                    ty: catch_ty.clone(),
                });
                for s in catch_body {
                    self.visit_stmt(s);
                }
                self.locals.truncate(saved);
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn push_local_binding(
        &mut self,
        name: &str,
        ty: Option<Type>,
        init: Option<&Expr>,
        stmt_span: &Range<usize>,
    ) {
        let resolved = ty.unwrap_or_else(|| match init {
            Some(e) => self.infer_local_ty(e),
            None => Type::Named("any".into()),
        });
        let span = locate_word_in(self.source, stmt_span, name).unwrap_or_else(|| stmt_span.clone());
        self.locals.push(LocalBind {
            name: name.to_string(),
            def_span: span,
            ty: resolved,
        });
    }

    fn push_locals_from_stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Local { name, ty, value } => {
                self.push_local_binding(name, ty.clone(), value.as_ref().map(|v| &v.value), &s.span);
            }
            Stmt::LocalMulti { names, values } => {
                for (i, (name, ty)) in names.iter().enumerate() {
                    let init = values.get(i).map(|v| &v.value);
                    self.push_local_binding(name, ty.clone(), init, &s.span);
                }
            }
            _ => {}
        }
    }

    fn with_block(&mut self, body: impl FnOnce(&mut Self)) {
        let saved = self.locals.len();
        body(self);
        self.locals.truncate(saved);
    }

    fn visit_decl(&mut self, d: &Spanned<Decl>) {
        if !contains(&d.span, self.offset) {
            return;
        }
        match &d.value {
            Decl::Function {
                name,
                params,
                body,
                ..
            } => {
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Function(name.clone()));
                }
                self.enter_function(params, |this| {
                    for p in params {
                        if let Some(span) = locate_word_in(this.source, &p.span, &p.name) {
                            this.record(span.clone(), Symbol::Local {
                                name: p.name.clone(),
                                def_span: span,
                            });
                        }
                        if let Some(def) = &p.default {
                            this.visit_expr(def);
                        }
                    }
                    for s in body {
                        this.visit_stmt(s);
                    }
                });
            }
            Decl::Class { name, members, .. } => {
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Class(name.clone()));
                }
                let prev = self.enclosing_class.replace(name.clone());
                for m in members {
                    self.visit_member(m);
                }
                self.enclosing_class = prev;
            }
            Decl::Interface { name, .. } => {
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Interface(name.clone()));
                }
            }
            Decl::Enum { name, variants, methods, .. } => {
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Enum(name.clone()));
                }
                for v in variants {
                    let (vname, fields) = match &v.value {
                        EnumVariant::Bare(n) | EnumVariant::Valued(n, _) => (n.as_str(), None),
                        EnumVariant::Tuple { name, fields } => (name.as_str(), Some(fields)),
                    };
                    if let Some(span) = locate_word_in(self.source, &v.span, vname) {
                        self.record(span, Symbol::EnumVariant {
                            enum_name: name.clone(),
                            variant: vname.to_string(),
                        });
                    }
                    if let Some(fs) = fields {
                        for p in fs {
                            if let Some(def) = &p.default {
                                self.visit_expr(def);
                            }
                        }
                    }
                    if let EnumVariant::Valued(_, e) = &v.value {
                        self.visit_expr(e);
                    }
                }
                let prev = self.enclosing_class.replace(name.clone());
                for m in methods {
                    self.visit_method(m, name);
                }
                self.enclosing_class = prev;
            }
            Decl::Import { path, .. } => {
                // Find the path literal inside the import statement —
                // the path appears between the matching quotes.
                if let Some(span) = locate_string_literal(self.source, &d.span, path) {
                    self.record(span, Symbol::ImportPath(path.clone()));
                }
            }
        }
    }

    fn visit_member(&mut self, m: &Spanned<ClassMember>) {
        if !contains(&m.span, self.offset) {
            return;
        }
        let class = self.enclosing_class.clone().unwrap_or_default();
        match &m.value {
            ClassMember::Field { name, default, .. } => {
                if let Some(span) = locate_word_in(self.source, &m.span, name) {
                    self.record(span, Symbol::Field {
                        class: class.clone(),
                        name: name.clone(),
                    });
                }
                if let Some(def) = default {
                    self.visit_expr(def);
                }
            }
            ClassMember::Method(meth) => self.visit_method(meth, &class),
        }
    }

    fn visit_method(&mut self, meth: &Method, class: &str) {
        if !contains(&meth.span, self.offset) {
            return;
        }
        if let Some(span) = locate_word_in(self.source, &meth.span, &meth.name) {
            self.record(span, Symbol::Method {
                class: class.to_string(),
                name: meth.name.clone(),
            });
        }
        self.enter_function(&meth.params, |this| {
            for p in &meth.params {
                if let Some(span) = locate_word_in(this.source, &p.span, &p.name) {
                    this.record(span.clone(), Symbol::Local {
                        name: p.name.clone(),
                        def_span: span,
                    });
                }
                if let Some(def) = &p.default {
                    this.visit_expr(def);
                }
            }
            for s in &meth.body {
                this.visit_stmt(s);
            }
        });
    }

    fn visit_expr(&mut self, e: &Spanned<Expr>) {
        if !contains(&e.span, self.offset) {
            return;
        }
        match &e.value {
            Expr::Ident(name) => {
                if let Some(local) = self.lookup_local(name) {
                    self.record(e.span.clone(), Symbol::Local {
                        name: name.clone(),
                        def_span: local.def_span.clone(),
                    });
                } else if with_classes(|r| r.contains_key(name)) {
                    self.record(e.span.clone(), Symbol::Class(name.clone()));
                } else if with_interfaces(|r| r.contains_key(name)) {
                    self.record(e.span.clone(), Symbol::Interface(name.clone()));
                } else if with_enums(|r| r.contains_key(name)) {
                    self.record(e.span.clone(), Symbol::Enum(name.clone()));
                } else if !saule_typeck::sigs::is_module(name)
                    && !saule_typeck::sigs::is_value_type(name)
                    && saule_typeck::sigs::lookup(name).is_none()
                {
                    // Unrecognised — assume free function declared in
                    // this workspace.
                    self.record(e.span.clone(), Symbol::Function(name.clone()));
                }
            }
            Expr::Self_ => {
                if let Some(class) = self.enclosing_class.clone() {
                    self.record(e.span.clone(), Symbol::Class(class));
                }
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                if let Some(span) =
                    member_name_span(self.source, obj.span.end, e.span.end, name)
                    && contains(&span, self.offset)
                {
                    if let Some(class) = self.receiver_class(&obj.value) {
                        // Enum.Variant access (no payload) — surface as
                        // a variant reference rather than a field.
                        if with_enums(|r| {
                            r.get(&class)
                                .map(|info| info.variants.contains_key(name))
                                .unwrap_or(false)
                        }) {
                            self.record(
                                span,
                                Symbol::EnumVariant {
                                    enum_name: class,
                                    variant: name.clone(),
                                },
                            );
                            return;
                        }
                        // Static method access through a class name —
                        // treat as a method reference.
                        if lookup_method(&class, name).is_some() {
                            self.record(
                                span,
                                Symbol::Method {
                                    class,
                                    name: name.clone(),
                                },
                            );
                            return;
                        }
                        self.record(span, Symbol::Field {
                            class,
                            name: name.clone(),
                        });
                        return;
                    }
                    self.record(span, Symbol::Field {
                        class: String::new(),
                        name: name.clone(),
                    });
                    return;
                }
                self.visit_expr(obj);
            }
            Expr::Index { obj, index } => {
                self.visit_expr(obj);
                self.visit_expr(index);
            }
            Expr::Call { callee, args } => {
                self.visit_expr(callee);
                for a in args {
                    self.visit_call_arg(a);
                }
            }
            Expr::MethodCall { obj, method, args } => {
                if let Some(span) =
                    member_name_span(self.source, obj.span.end, e.span.end, method)
                    && contains(&span, self.offset)
                {
                    if let Some(class) = self.receiver_class(&obj.value) {
                        self.record(span, Symbol::Method {
                            class,
                            name: method.clone(),
                        });
                    }
                    return;
                }
                self.visit_expr(obj);
                for a in args {
                    self.visit_call_arg(a);
                }
            }
            Expr::Unary { rhs, .. } => self.visit_expr(rhs),
            Expr::Binary { lhs, rhs, .. } => {
                self.visit_expr(lhs);
                self.visit_expr(rhs);
            }
            Expr::ForceUnwrap(inner) => self.visit_expr(inner),
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        TableEntry::Positional(v) => self.visit_expr(v),
                        TableEntry::Field { key, value } => {
                            self.visit_expr(key);
                            self.visit_expr(value);
                        }
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                let params_clone = params.clone();
                self.enter_function(&params_clone, |this| match body {
                    LambdaBody::Expr(b) => this.visit_expr(b),
                    LambdaBody::Block(b) => {
                        for s in b {
                            this.visit_stmt(s);
                        }
                    }
                });
            }
            Expr::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                let scrut_ty = inferred_type_of(&scrutinee.value, &self.locals, &self.enclosing_class);
                for arm in arms {
                    let saved = self.locals.len();
                    self.bind_pattern(&arm.pattern, scrut_ty.as_ref());
                    self.visit_pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    match &arm.body {
                        MatchBody::Expr(e) => self.visit_expr(e),
                        MatchBody::Block(b) => {
                            for s in b {
                                self.visit_stmt(s);
                            }
                        }
                    }
                    self.locals.truncate(saved);
                }
            }
            Expr::Pipe { source, stages } => {
                self.visit_expr(source);
                for st in stages {
                    for a in &st.args {
                        self.visit_call_arg(a);
                    }
                }
            }
            Expr::Int(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Str(_) | Expr::Nil => {}
        }
    }

    fn visit_call_arg(&mut self, a: &CallArg) {
        match a {
            CallArg::Positional(e) => self.visit_expr(e),
            CallArg::Named { value, .. } => self.visit_expr(value),
        }
    }

    fn visit_pattern(&mut self, p: &Spanned<Pattern>) {
        if !contains(&p.span, self.offset) {
            return;
        }
        match &p.value {
            Pattern::Variant { enum_name, variant, fields } => {
                // Enum name and variant name appear in source as
                // `Enum.Variant(...)`. Locate each within the pattern
                // span; the enum name comes first, then the variant
                // after the dot.
                if let Some(espan) = locate_word_in(self.source, &p.span, enum_name)
                    && contains(&espan, self.offset)
                {
                    self.record(espan, Symbol::Enum(enum_name.clone()));
                    return;
                }
                let after_enum = p.span.start
                    + enum_name.len()
                    + 1; // dot
                let lookup_range = after_enum..p.span.end;
                if let Some(vspan) = locate_word_in(self.source, &lookup_range, variant)
                    && contains(&vspan, self.offset)
                {
                    self.record(vspan, Symbol::EnumVariant {
                        enum_name: enum_name.clone(),
                        variant: variant.clone(),
                    });
                    return;
                }
                for f in fields {
                    self.visit_pattern(f);
                }
            }
            Pattern::Tuple(parts) => {
                for f in parts {
                    self.visit_pattern(f);
                }
            }
            Pattern::Bind(name) => {
                if let Some(span) = locate_word_in(self.source, &p.span, name)
                    && contains(&span, self.offset)
                {
                    self.record(span.clone(), Symbol::Local {
                        name: name.clone(),
                        def_span: span,
                    });
                }
            }
            _ => {}
        }
    }

    fn bind_pattern(&mut self, pat: &Spanned<Pattern>, scrut_ty: Option<&Type>) {
        match &pat.value {
            Pattern::Bind(name) => {
                let ty = scrut_ty
                    .cloned()
                    .map(strip_nullable)
                    .unwrap_or_else(|| Type::Named("any".into()));
                let span = locate_word_in(self.source, &pat.span, name)
                    .unwrap_or_else(|| pat.span.clone());
                self.locals.push(LocalBind {
                    name: name.clone(),
                    def_span: span,
                    ty,
                });
            }
            Pattern::Variant { fields, .. } | Pattern::Tuple(fields) => {
                for sub in fields {
                    self.bind_pattern(sub, None);
                }
            }
            _ => {}
        }
    }
}
