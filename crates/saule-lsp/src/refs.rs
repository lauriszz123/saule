//! Symbol resolution + reference collection used by goto-definition
//! and find-references.
//!
//! Two passes share the same AST walk shape:
//!
//! * [`find_symbol_at`] — given a byte offset, identify what semantic
//!   symbol (class, method, field, function, enum variant, local,
//!   import path) the cursor is on.
//!
//! * [`collect_in_module`] — walk a module and emit every span where a
//!   given [`Symbol`] is defined or referenced.
//!
//! The AST stores names as bare `String`s without inner spans, so name
//! locations are recovered by scanning the source within the parent
//! node's span (e.g. `class Foo` → locate `Foo` after `class`). This is
//! good enough because identifier names parse as a single token whose
//! position inside a known structural span is unambiguous.

use std::ops::Range;

use saule_ast::{
    CallArg, ClassMember, Decl, EnumVariant, Expr, LambdaBody, MatchBody, Method,
    Module, Param, Pattern, Spanned, Stmt, TableEntry, Type,
};
use saule_semantic::{lookup_field_type, lookup_method, with_classes, with_enums, with_interfaces};

// ──────────────────────────────────────────────────────────────────────────────
// Symbols
// ──────────────────────────────────────────────────────────────────────────────

/// A semantic symbol identified by name + kind.
///
/// Workspace-scoped variants (everything except `Local`) drive a
/// cross-file walk; `Local` is bounded to the file in which the
/// declaration lives and identified by the declaration's byte span so
/// shadowing inner-scope variables of the same name don't get
/// conflated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Symbol {
    Class(String),
    Interface(String),
    Enum(String),
    EnumVariant {
        enum_name: String,
        variant: String,
    },
    Function(String),
    Method {
        class: String,
        name: String,
    },
    Field {
        class: String,
        name: String,
    },
    /// Parameter, local variable, loop variable, catch variable, or
    /// match-arm binding. `def_span` is the byte range of the binding
    /// identifier itself (not the surrounding stmt) and serves as the
    /// stable identity of this local.
    Local {
        name: String,
        def_span: Range<usize>,
    },
    /// The string literal of an `import "path"` statement. Definition
    /// = the imported file's head; references = importer statements
    /// across the workspace.
    ImportPath(String),
}

impl Symbol {
    /// `true` when this symbol can have references in other files.
    #[allow(dead_code)]
    pub fn is_workspace(&self) -> bool {
        !matches!(self, Symbol::Local { .. })
    }
}

/// A resolved cursor target: which symbol it is, plus the source span
/// the cursor was on (so the editor can highlight the clicked name).
#[derive(Debug, Clone)]
pub struct Resolved {
    pub symbol: Symbol,
    pub span: Range<usize>,
}

/// One occurrence of a symbol within a module. `is_def` flags the
/// declaring site so callers can prefer it for goto-definition or
/// exclude/include it from a references list per the LSP spec.
#[derive(Debug, Clone)]
pub struct Hit {
    pub span: Range<usize>,
    pub is_def: bool,
}

// ──────────────────────────────────────────────────────────────────────────────
// Public entry points
// ──────────────────────────────────────────────────────────────────────────────

/// Resolve the cursor at `offset` to a [`Symbol`]. Returns `None` for
/// positions on whitespace / literals / unknown identifiers.
pub fn find_symbol_at(module: &Module, source: &str, offset: usize) -> Option<Resolved> {
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

/// Walk `module` and emit every span that defines or references
/// `symbol`. The order follows the AST traversal — the LSP layer
/// canonicalises duplicates if any.
pub fn collect_in_module(module: &Module, source: &str, symbol: &Symbol) -> Vec<Hit> {
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

// ──────────────────────────────────────────────────────────────────────────────
// Source-scanning helpers
// ──────────────────────────────────────────────────────────────────────────────

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Locate the first occurrence of `name` as a word (bounded by
/// non-identifier bytes) inside `source[range]`. Returns the absolute
/// byte range of the match, or `None` if `name` doesn't appear there
/// as a standalone identifier.
fn locate_word_in(source: &str, range: &Range<usize>, name: &str) -> Option<Range<usize>> {
    let end = range.end.min(source.len());
    let start = range.start.min(end);
    let slice = source.get(start..end)?;
    let bytes = slice.as_bytes();
    let pat = name.as_bytes();
    if pat.is_empty() || pat.len() > bytes.len() {
        return None;
    }
    let mut i = 0;
    while i + pat.len() <= bytes.len() {
        if &bytes[i..i + pat.len()] == pat {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok = i + pat.len() == bytes.len() || !is_ident_byte(bytes[i + pat.len()]);
            if before_ok && after_ok {
                return Some((start + i)..(start + i + pat.len()));
            }
        }
        i += 1;
    }
    None
}

/// Find every occurrence of `name` as a word inside `source[range]`.
/// Used for collecting references in spans that contain multiple uses
/// (e.g. an entire module body for a workspace search).
fn locate_words_in(source: &str, range: &Range<usize>, name: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let end = range.end.min(source.len());
    let start = range.start.min(end);
    let Some(slice) = source.get(start..end) else {
        return out;
    };
    let bytes = slice.as_bytes();
    let pat = name.as_bytes();
    if pat.is_empty() || pat.len() > bytes.len() {
        return out;
    }
    let mut i = 0;
    while i + pat.len() <= bytes.len() {
        if &bytes[i..i + pat.len()] == pat {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok = i + pat.len() == bytes.len() || !is_ident_byte(bytes[i + pat.len()]);
            if before_ok && after_ok {
                out.push((start + i)..(start + i + pat.len()));
                i += pat.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

fn contains(r: &Range<usize>, o: usize) -> bool {
    r.start <= o && o <= r.end
}

/// Span of a member access's `.name` part: starts after `obj.span.end`
/// (which sits on or just after the dot). Falls back to a search
/// within a wider window when the member lies on a subsequent line
/// after a multi-line `obj` expression.
fn member_name_span(
    source: &str,
    obj_end: usize,
    parent_end: usize,
    name: &str,
) -> Option<Range<usize>> {
    let range = obj_end..parent_end.max(obj_end);
    locate_word_in(source, &range, name)
}

// ──────────────────────────────────────────────────────────────────────────────
// Resolve cursor → Symbol
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct LocalBind {
    name: String,
    /// Byte span of the binding identifier as it appears at its
    /// declaration site. Stable identity for this local across both
    /// resolution and collection.
    def_span: Range<usize>,
    ty: Type,
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

// ──────────────────────────────────────────────────────────────────────────────
// Collect references to a known Symbol
// ──────────────────────────────────────────────────────────────────────────────

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
            Expr::MethodCall { obj: inner, method, .. } => {
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
            let def_span =
                locate_word_in(self.source, &p.span, &p.name).unwrap_or(p.span.clone());
            // Param binding sites aren't a Local "definition" in the
            // referencing-search sense unless the target Symbol is
            // exactly this binding — handled in `visit_param_binding`.
            if let Symbol::Local { name, def_span: target_def } = self.symbol
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
            Stmt::Local { name, ty, value } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                let def_span = locate_word_in(self.source, &s.span, name);
                if let Some(span) = &def_span {
                    if let Symbol::Local { name: tname, def_span: tspan } = self.symbol
                        && tname == name
                        && tspan == span
                    {
                        self.push(span.clone(), true);
                    }
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
                for (i, (name, ty)) in names.iter().enumerate() {
                    let def_span = locate_word_in(self.source, &s.span, name);
                    if let Some(span) = &def_span {
                        if let Symbol::Local { name: tname, def_span: tspan } = self.symbol
                            && tname == name
                            && tspan == span
                        {
                            self.push(span.clone(), true);
                        }
                    }
                    let resolved = ty.clone().unwrap_or_else(|| match values.get(i) {
                        Some(v) => self.infer_local_ty(&v.value),
                        None => Type::Named("any".into()),
                    });
                    self.locals.push(LocalBind {
                        name: name.clone(),
                        def_span: def_span.unwrap_or_else(|| s.span.clone()),
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
                let span = locate_word_in(self.source, &s.span, var)
                    .unwrap_or_else(|| s.span.clone());
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
                    if let Symbol::Local { name: tname, def_span } = self.symbol
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
                name,
                params,
                body,
                ..
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
            Decl::Enum { name, variants, methods, .. } => {
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
                    if let Symbol::EnumVariant { enum_name: en, variant } = self.symbol
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
            Decl::Import { path, .. } => {
                if let Symbol::ImportPath(target) = self.symbol
                    && target == path
                    && let Some(span) = locate_string_literal(self.source, &d.span, path)
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
            Decl::Class { members, .. } => members
                .first()
                .map(|m| m.span.start)
                .unwrap_or(d.span.end),
            Decl::Interface { methods, .. } => methods
                .first()
                .map(|m| m.span.start)
                .unwrap_or(d.span.end),
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
                if let Symbol::Field { class: tc, name: tn } = self.symbol
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
        if let Symbol::Method { class: tc, name: tn } = self.symbol
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
            Expr::Ident(name) => match self.symbol {
                Symbol::Local { name: tname, def_span } => {
                    if name == tname
                        && self.lookup_local(name).is_some_and(|l| &l.def_span == def_span)
                    {
                        self.push(e.span.clone(), false);
                    }
                }
                Symbol::Class(t) | Symbol::Interface(t) | Symbol::Enum(t) | Symbol::Function(t) => {
                    if name == t && self.lookup_local(name).is_none() {
                        self.push(e.span.clone(), false);
                    }
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
                let class = self.receiver_class(&obj.value);
                match self.symbol {
                    Symbol::Field { class: tc, name: tn } => {
                        if name == tn && class.as_deref() == Some(tc.as_str()) {
                            self.push(span, false);
                        }
                    }
                    Symbol::Method { class: tc, name: tn } => {
                        if name == tn && class.as_deref() == Some(tc.as_str()) {
                            self.push(span, false);
                        }
                    }
                    Symbol::EnumVariant { enum_name, variant } => {
                        if name == variant && class.as_deref() == Some(enum_name.as_str()) {
                            self.push(span, false);
                        }
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
                if let Symbol::Method { class: tc, name: tn } = self.symbol
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
        match &p.value {
            Pattern::Variant { enum_name, variant, fields } => {
                match self.symbol {
                    Symbol::Enum(t) if t == enum_name => {
                        if let Some(span) = locate_word_in(self.source, &p.span, enum_name) {
                            self.push(span, false);
                        }
                    }
                    Symbol::EnumVariant { enum_name: tn, variant: tv }
                        if tn == enum_name && tv == variant =>
                    {
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

fn declared_name(d: &Decl) -> &str {
    match d {
        Decl::Function { name, .. }
        | Decl::Class { name, .. }
        | Decl::Interface { name, .. }
        | Decl::Enum { name, .. } => name,
        Decl::Import { .. } => "",
    }
}

fn named_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => named_type(inner),
        _ => None,
    }
}

fn strip_nullable(ty: Type) -> Type {
    match ty {
        Type::Nullable(inner) => *inner,
        other => other,
    }
}

/// Lightweight inference for a scrutinee expression, mirroring the
/// hover module's logic. Only the cases needed for typing match-arm
/// bindings are covered — anything else falls through to `None`.
fn inferred_type_of(
    init: &Expr,
    locals: &[LocalBind],
    enclosing_class: &Option<String>,
) -> Option<Type> {
    match init {
        Expr::Self_ => enclosing_class.as_ref().map(|c| Type::Named(c.clone())),
        Expr::Ident(n) => locals.iter().rev().find(|l| &l.name == n).map(|l| l.ty.clone()),
        Expr::Call { callee, .. } => {
            if let Expr::Ident(n) = &callee.value
                && with_classes(|r| r.contains_key(n))
            {
                return Some(Type::Named(n.clone()));
            }
            None
        }
        _ => None,
    }
}

/// Find the byte range of `path`'s string-literal occurrence inside an
/// `import "…"` statement. Looks for the first quote after the start
/// of `range`, then matches a closing quote at `start + path.len()+1`.
fn locate_string_literal(
    source: &str,
    range: &Range<usize>,
    path: &str,
) -> Option<Range<usize>> {
    let bytes = source.as_bytes();
    let end = range.end.min(bytes.len());
    let mut i = range.start.min(end);
    while i < end {
        if bytes[i] == b'"' {
            let start = i + 1;
            let stop = start + path.len();
            if stop < bytes.len() && bytes[stop] == b'"' {
                let candidate = source.get(start..stop)?;
                if candidate == path {
                    return Some(start..stop);
                }
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    fn init_stdlib() {
        static ONCE: Once = Once::new();
        ONCE.call_once(saule_interpreter::init);
    }

    fn parse_src(src: &str) -> Module {
        let toks = saule_lexer::Lexer::new(src).tokenize().expect("lex");
        saule_parser::parse(toks).expect("parse")
    }

    fn analyze(module: &Module) {
        let _ = saule_semantic::analyze(module);
    }

    /// Resolve at the byte offset of the middle of `needle`'s first
    /// occurrence in `src`.
    fn resolve(src: &str, needle: &str) -> Symbol {
        init_stdlib();
        let module = parse_src(src);
        analyze(&module);
        let off = src.find(needle).expect("needle") + needle.len() / 2;
        find_symbol_at(&module, src, off)
            .unwrap_or_else(|| panic!("no symbol at {needle:?}"))
            .symbol
    }

    fn defs_and_refs(src: &str, sym: &Symbol) -> Vec<Hit> {
        let module = parse_src(src);
        analyze(&module);
        collect_in_module(&module, src, sym)
    }

    #[test]
    fn resolves_top_level_function_at_call_site() {
        let src = "fn add(a: integer) -> integer\n  return a\nend\nfn main() -> integer\n  return add(1)\nend\n";
        let s = resolve(src, "add(1)");
        assert!(matches!(&s, Symbol::Function(n) if n == "add"));
        let hits = defs_and_refs(src, &s);
        assert_eq!(hits.iter().filter(|h| h.is_def).count(), 1);
        assert_eq!(hits.iter().filter(|h| !h.is_def).count(), 1);
    }

    #[test]
    fn resolves_local_only_within_file() {
        let src = "fn main() -> integer\n  local x: integer = 1\n  return x + x\nend\n";
        let s = resolve(src, "x:");
        assert!(matches!(&s, Symbol::Local { name, .. } if name == "x"));
        assert!(!s.is_workspace());
        let hits = defs_and_refs(src, &s);
        assert_eq!(hits.iter().filter(|h| h.is_def).count(), 1);
        assert_eq!(hits.iter().filter(|h| !h.is_def).count(), 2);
    }
}
