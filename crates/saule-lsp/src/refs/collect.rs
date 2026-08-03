//! Reference collector — walks the AST and emits every span that
//! defines or references a given [`Symbol`]. Used by find-references.

use std::ops::Range;

use saule_ast::{
    CallArg, ClassMember, Decl, EnumVariant, Expr, LambdaBody, MatchBody, Method, Module, Param,
    Pattern, Spanned, Stmt, TableEntry, Type,
};
use saule_semantic::{
    lookup_field_type, lookup_method, super_init_target, with_classes, with_enums,
};

use super::util::{
    LocalBind, declared_name, inferred_type_of, locate_import_path, locate_word_in,
    locate_words_in, member_name_span, named_type, strip_nullable,
};
use super::{Hit, Symbol};

pub(super) fn run(module: &Module, source: &str, symbol: &Symbol) -> Vec<Hit> {
    let mut cx = CollectCx {
        source,
        symbol,
        enclosing_class: None,
        locals: Vec::new(),
        out: Vec::new(),
    };
    cx.visit_module(module);
    cx.out
}

struct CollectCx<'a> {
    source: &'a str,
    symbol: &'a Symbol,
    enclosing_class: Option<String>,
    locals: Vec<LocalBind>,
    out: Vec<Hit>,
}

impl<'a> CollectCx<'a> {
    fn lookup_local(&self, name: &str) -> Option<&LocalBind> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

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
                let ic = self.receiver_class(&inner.value)?;
                let ty = lookup_field_type(&ic, name)?;
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
            Expr::MethodCall {
                obj: inner, method, ..
            } => {
                let ic = self.receiver_class(&inner.value)?;
                let sig = lookup_method(&ic, method)?;
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

    fn push(&mut self, span: Range<usize>, is_def: bool) {
        self.out.push(Hit { span, is_def });
    }

    fn enter_function(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let saved = std::mem::take(&mut self.locals);
        for p in params {
            let def_span = locate_word_in(self.source, &p.span, &p.name).unwrap_or(p.span.clone());
            // Param binding sites aren't a Local "definition" in the
            // referencing-search sense unless the target Symbol is
            // exactly this binding — handled in `visit_param_binding`.
            if let Symbol::Local {
                name,
                def_span: target_def,
            } = self.symbol
                && name == &p.name
                && target_def == &def_span
            {
                self.push(def_span.clone(), true);
            }
            self.locals.push(LocalBind {
                name: p.name.clone(),
                def_span,
                ty: p.ty.clone(),
            });
        }
        body(self);
        self.locals = saved;
    }

    fn visit_module(&mut self, m: &Module) {
        for s in &m.stmts {
            self.visit_stmt(s);
        }
    }

    fn visit_stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Decl(d) => self.visit_decl(d),
            Stmt::Local {
                name, ty, value, ..
            } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                let def_span = locate_word_in(self.source, &s.span, name);
                if let Some(span) = &def_span
                    && let Symbol::Local {
                        name: tname,
                        def_span: tspan,
                    } = self.symbol
                    && tname == name
                    && tspan == span
                {
                    self.push(span.clone(), true);
                }
                let resolved = ty.clone().unwrap_or_else(|| match value {
                    Some(v) => self.infer_local_ty(&v.value),
                    None => Type::Named("any".into()),
                });
                self.locals.push(LocalBind {
                    name: name.clone(),
                    def_span: def_span.unwrap_or_else(|| s.span.clone()),
                    ty: resolved,
                });
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.visit_expr(v);
                }
                for (i, (name, name_span, ty)) in names.iter().enumerate() {
                    if let Symbol::Local {
                        name: tname,
                        def_span: tspan,
                    } = self.symbol
                        && tname == name
                        && tspan == name_span
                    {
                        self.push(name_span.clone(), true);
                    }
                    let resolved = ty.clone().unwrap_or_else(|| match values.get(i) {
                        Some(v) => self.infer_local_ty(&v.value),
                        None => Type::Named("any".into()),
                    });
                    self.locals.push(LocalBind {
                        name: name.clone(),
                        def_span: name_span.clone(),
                        ty: resolved,
                    });
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
                let span =
                    locate_word_in(self.source, &s.span, var).unwrap_or_else(|| s.span.clone());
                if let Symbol::Local { name, def_span } = self.symbol
                    && name == var
                    && def_span == &span
                {
                    self.push(span.clone(), true);
                }
                let saved = self.locals.len();
                self.locals.push(LocalBind {
                    name: var.clone(),
                    def_span: span,
                    ty: var_ty
                        .clone()
                        .unwrap_or_else(|| Type::Named("integer".into())),
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
                    if let Symbol::Local {
                        name: tname,
                        def_span,
                    } = self.symbol
                        && tname == name
                        && def_span == &span
                    {
                        self.push(span.clone(), true);
                    }
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
                if let Symbol::Local { name, def_span } = self.symbol
                    && name == catch_var
                    && def_span == &span
                {
                    self.push(span.clone(), true);
                }
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

    fn with_block(&mut self, body: impl FnOnce(&mut Self)) {
        let saved = self.locals.len();
        body(self);
        self.locals.truncate(saved);
    }

    fn visit_decl(&mut self, d: &Spanned<Decl>) {
        match &d.value {
            Decl::Function {
                name, params, body, ..
            } => {
                if let Symbol::Function(target) = self.symbol
                    && target == name
                    && let Some(span) = locate_word_in(self.source, &d.span, name)
                {
                    self.push(span, true);
                }
                self.enter_function(params, |this| {
                    for p in params {
                        if let Some(def) = &p.default {
                            this.visit_expr(def);
                        }
                    }
                    for s in body {
                        this.visit_stmt(s);
                    }
                });
            }
            Decl::Class {
                name,
                extends,
                implements,
                members,
                ..
            } => {
                if let Symbol::Class(target) = self.symbol
                    && target == name
                    && let Some(span) = locate_word_in(self.source, &d.span, name)
                {
                    self.push(span, true);
                }
                // `extends` / `implements` references
                self.collect_type_name_refs_in_header(d, extends.as_deref(), implements);
                let prev = self.enclosing_class.replace(name.clone());
                for m in members {
                    self.visit_member(m);
                }
                self.enclosing_class = prev;
            }
            Decl::Interface {
                name,
                extends,
                methods,
                ..
            } => {
                if let Symbol::Interface(target) = self.symbol
                    && target == name
                    && let Some(span) = locate_word_in(self.source, &d.span, name)
                {
                    self.push(span, true);
                }
                self.collect_type_name_refs_in_header(d, None, extends);
                // Method-sig param/return type references handled via
                // a span-bounded source scan inside the decl head.
                let _ = methods;
            }
            Decl::Enum {
                name,
                variants,
                methods,
                ..
            } => {
                if let Symbol::Enum(target) = self.symbol
                    && target == name
                    && let Some(span) = locate_word_in(self.source, &d.span, name)
                {
                    self.push(span, true);
                }
                for v in variants {
                    let vname = match &v.value {
                        EnumVariant::Bare(n) | EnumVariant::Valued(n, _) => n,
                        EnumVariant::Tuple { name, .. } => name,
                    };
                    if let Symbol::EnumVariant {
                        enum_name: en,
                        variant,
                    } = self.symbol
                        && en == name
                        && variant == vname
                        && let Some(span) = locate_word_in(self.source, &v.span, vname)
                    {
                        self.push(span, true);
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
            Decl::Import { path, quoted, .. } => {
                if let Symbol::ImportPath(target) = self.symbol
                    && target == path
                    && let Some(span) = locate_import_path(self.source, &d.span, path, *quoted)
                {
                    self.push(span, false);
                }
            }
        }
    }

    /// Class / Interface header scan: collect references to a target
    /// class/interface name appearing in `extends X` or
    /// `implements A, B, C`. We bound the search to the source slice
    /// from the start of the decl up to (but not including) the first
    /// member, where headers always live.
    fn collect_type_name_refs_in_header(
        &mut self,
        d: &Spanned<Decl>,
        extends: Option<&str>,
        implements: &[String],
    ) {
        let target_name = match self.symbol {
            Symbol::Class(n) | Symbol::Interface(n) | Symbol::Enum(n) => n,
            _ => return,
        };
        let head_end = match &d.value {
            Decl::Class { members, .. } => {
                members.first().map(|m| m.span.start).unwrap_or(d.span.end)
            }
            Decl::Interface { methods, .. } => {
                methods.first().map(|m| m.span.start).unwrap_or(d.span.end)
            }
            _ => d.span.end,
        };
        let head = d.span.start..head_end;
        let in_extends = extends == Some(target_name);
        let in_implements = implements.iter().any(|n| n == target_name);
        if !in_extends && !in_implements {
            return;
        }
        for span in locate_words_in(self.source, &head, target_name) {
            // Skip the decl-name occurrence we already handled.
            if let Some(name_span) = locate_word_in(self.source, &d.span, declared_name(&d.value))
                && name_span == span
            {
                continue;
            }
            self.push(span, false);
        }
    }

    fn visit_member(&mut self, m: &Spanned<ClassMember>) {
        let class = self.enclosing_class.clone().unwrap_or_default();
        match &m.value {
            ClassMember::Field { name, default, .. } => {
                if let Symbol::Field {
                    class: tc,
                    name: tn,
                } = self.symbol
                    && tc == &class
                    && tn == name
                    && let Some(span) = locate_word_in(self.source, &m.span, name)
                {
                    self.push(span, true);
                }
                if let Some(def) = default {
                    self.visit_expr(def);
                }
            }
            ClassMember::Method(meth) => self.visit_method(meth, &class),
        }
    }

    fn visit_method(&mut self, meth: &Method, class: &str) {
        if let Symbol::Method {
            class: tc,
            name: tn,
        } = self.symbol
            && tc == class
            && tn == &meth.name
            && let Some(span) = locate_word_in(self.source, &meth.span, &meth.name)
        {
            self.push(span, true);
        }
        self.enter_function(&meth.params, |this| {
            for p in &meth.params {
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
        match &e.value {
            Expr::Cast { value, .. } => self.visit_expr(value),
            Expr::Ident(name) => match self.symbol {
                Symbol::Local {
                    name: tname,
                    def_span,
                } => {
                    if name == tname
                        && self
                            .lookup_local(name)
                            .is_some_and(|l| &l.def_span == def_span)
                    {
                        self.push(e.span.clone(), false);
                    }
                }
                Symbol::Class(t) | Symbol::Interface(t) | Symbol::Enum(t) | Symbol::Function(t)
                    if name == t && self.lookup_local(name).is_none() =>
                {
                    self.push(e.span.clone(), false);
                }
                _ => {}
            },
            Expr::Self_ => {}
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                self.visit_expr(obj);
                let Some(span) = member_name_span(self.source, obj.span.end, e.span.end, name)
                else {
                    return;
                };
                // `self.super(...)` is a reference to the parent
                // constructor, not to a member named `super` — mirror
                // what the cursor resolver records for it.
                if name == "super"
                    && matches!(obj.value, Expr::Self_)
                    && let Symbol::Method {
                        class: tc,
                        name: tn,
                    } = self.symbol
                    && tn == "init"
                    && let Some(enclosing) = &self.enclosing_class
                    && let Some((owner, _)) = super_init_target(enclosing)
                    && &owner == tc
                {
                    self.push(span, false);
                    return;
                }
                let class = self.receiver_class(&obj.value);
                match self.symbol {
                    Symbol::Field {
                        class: tc,
                        name: tn,
                    } => {
                        if name == tn && class.as_deref() == Some(tc.as_str()) {
                            self.push(span, false);
                        }
                    }
                    Symbol::Method {
                        class: tc,
                        name: tn,
                    } => {
                        if name == tn && class.as_deref() == Some(tc.as_str()) {
                            self.push(span, false);
                        }
                    }
                    Symbol::EnumVariant { enum_name, variant }
                        if name == variant && class.as_deref() == Some(enum_name.as_str()) =>
                    {
                        self.push(span, false);
                    }
                    _ => {}
                }
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
                self.visit_expr(obj);
                if let Symbol::Method {
                    class: tc,
                    name: tn,
                } = self.symbol
                    && method == tn
                    && let Some(class) = self.receiver_class(&obj.value)
                    && &class == tc
                    && let Some(span) =
                        member_name_span(self.source, obj.span.end, e.span.end, method)
                {
                    self.push(span, false);
                }
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
                        for s in b.iter() {
                            this.visit_stmt(s);
                        }
                    }
                });
            }
            Expr::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                let scrut_ty =
                    inferred_type_of(&scrutinee.value, &self.locals, &self.enclosing_class);
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
        match &p.value {
            Pattern::Variant {
                enum_name,
                variant,
                fields,
            } => {
                match self.symbol {
                    Symbol::Enum(t) if t == enum_name => {
                        if let Some(span) = locate_word_in(self.source, &p.span, enum_name) {
                            self.push(span, false);
                        }
                    }
                    Symbol::EnumVariant {
                        enum_name: tn,
                        variant: tv,
                    } if tn == enum_name && tv == variant => {
                        let after = p.span.start + enum_name.len() + 1;
                        let lookup = after..p.span.end;
                        if let Some(span) = locate_word_in(self.source, &lookup, variant) {
                            self.push(span, false);
                        }
                    }
                    _ => {}
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
                if let Symbol::Local { name: tn, def_span } = self.symbol
                    && tn == name
                    && let Some(span) = locate_word_in(self.source, &p.span, name)
                    && &span == def_span
                {
                    self.push(span, true);
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

// ──────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ──────────────────────────────────────────────────────────────────────────────
