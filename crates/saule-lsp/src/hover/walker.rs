//! AST walker that picks the smallest enclosing node at a byte offset
//! and produces a Markdown blurb for it. The big stateful machinery
//! (`Cx`, scope tracking, every `visit_*` arm) lives here; rendering
//! helpers live in [`super::render`] and shared utilities in
//! [`super::util`].

use std::collections::HashMap;
use std::ops::Range;

use saule_ast::{
    ClassMember, Decl, Expr, Method, Module, Param, Pattern, Spanned, Stmt, Type,
};
use saule_semantic::{lookup_field_type, lookup_method, with_classes, with_enums, with_interfaces};

use super::ImportContext;
use super::render::{
    collect_enum_variant_fields, render_class_full, render_class_head, render_enum_from_registry,
    render_enum_head, render_field, render_function_sig, render_interface_from_registry,
    render_interface_head, render_method_head, render_native_sig_full, render_param,
    render_stdlib_module, render_type, render_variant_pattern,
};
use super::util::{contains, named_type, resolve_member, strip_nullable_type};

/// Drive the hover walker against `module` for `offset` and return the
/// best (smallest-span) blurb we found, if any.
pub(super) fn run(
    module: &Module,
    offset: usize,
    imports: &ImportContext,
) -> Option<(String, Range<usize>)> {
    let mut cx = Cx {
        offset,
        enclosing_class: None,
        best: None,
        imports,
        locals: Vec::new(),
        enum_variant_fields: collect_enum_variant_fields(module),
    };
    cx.visit_module(module);
    cx.best.map(|h| (h.md, h.span))
}

struct Hit {
    span: Range<usize>,
    md: String,
}

/// One in-scope local binding (parameter, `local x =`, loop variable,
/// `try ... catch (e: T)` binding). Tracked as a flat stack — entering
/// a function/method/lambda saves the current stack and starts fresh,
/// exiting restores it. Block-level scoping inside a function is
/// approximated with a length-marker save/truncate idiom: precise
/// enough for hover, with no Vec<Vec<…>> overhead.
#[derive(Clone)]
struct LocalVar {
    name: String,
    ty: Type,
    kind: LocalKind,
}

#[derive(Clone, Copy)]
enum LocalKind {
    Param,
    Local,
    LoopVar,
    Catch,
    Binding,
}

struct Cx<'a> {
    offset: usize,
    enclosing_class: Option<String>,
    best: Option<Hit>,
    imports: &'a ImportContext,
    locals: Vec<LocalVar>,
    /// Tuple-variant payload field types, keyed by `(enum, variant)`.
    /// Populated once at the start of [`hover_at_with`] so pattern
    /// bindings inside `match` arms can be typed without re-walking
    /// every enum decl per arm.
    enum_variant_fields: HashMap<(String, String), Vec<Param>>,
}

impl<'a> Cx<'a> {
    /// Record `md` as the hover for `span` when `span` contains the
    /// cursor and is strictly narrower than any prior match.
    fn record(&mut self, span: Range<usize>, md: String) {
        if !contains(&span, self.offset) {
            return;
        }
        let new_w = span.end.saturating_sub(span.start);
        if let Some(b) = &self.best {
            let cur_w = b.span.end.saturating_sub(b.span.start);
            if new_w >= cur_w {
                return;
            }
        }
        self.best = Some(Hit { span, md });
    }

    /// Walk into a function/method/lambda body with a fresh local
    /// scope. Saves and restores the outer scope so a hover request
    /// inside a closure doesn't see locals from the enclosing function
    /// (which would be confusing) and vice versa.
    fn enter_function(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let saved = std::mem::take(&mut self.locals);
        for p in params {
            self.locals.push(LocalVar {
                name: p.name.clone(),
                ty: p.ty.clone(),
                kind: LocalKind::Param,
            });
        }
        body(self);
        self.locals = saved;
    }

    /// Look up `name` in the current local scope (innermost first).
    /// Returns `None` for free identifiers — the caller falls through
    /// to the registry / native-sig path.
    fn lookup_local(&self, name: &str) -> Option<&LocalVar> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

    /// Best-effort type inference for a `local x = <init>` site when
    /// the user didn't write an annotation. Handles the cases that
    /// account for the bulk of real-world `local`s in Saule code:
    ///
    /// * `Class(args)` — constructor call returns `Class`.
    /// * `obj:method(args)` — uses the registered method's return type.
    /// * `obj.field` — uses the field's declared type.
    /// * Existing local — propagates its known type.
    /// * `self` inside a method — the enclosing class.
    /// * Literal expressions — their primitive type.
    ///
    /// Anything else returns `None`; the caller falls back to `any`.
    fn infer_init_type(&self, init: &Expr) -> Option<Type> {
        match init {
            Expr::Call { callee, .. } => {
                if let Expr::Ident(name) = &callee.value {
                    if with_classes(|r| r.contains_key(name)) {
                        return Some(Type::Named(name.clone()));
                    }
                    // Non-constructor free call: consult native-sig
                    // returns or imported function signatures. We
                    // don't have ASTs for those, so return None and
                    // accept `any`.
                    if let Some(sig) = saule_typeck::sigs::lookup(name) {
                        return sig.returns.first().cloned();
                    }
                }
                // `recv.method(args)` — dot-call on an instance or
                // module. Resolve the receiver's class and chase the
                // method's return type the same way `MethodCall` does.
                if let Expr::Member { obj, name } = &callee.value {
                    let class = self.receiver_class(&obj.value)?;
                    if let Some(sig) = lookup_method(&class, name) {
                        return sig.return_ty;
                    }
                    let qname = format!("{class}.{name}");
                    if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
                        return sig.returns.first().cloned();
                    }
                }
                None
            }
            Expr::MethodCall { obj, method, .. } => {
                let class = self.receiver_class(&obj.value)?;
                if let Some(sig) = lookup_method(&class, method) {
                    return sig.return_ty;
                }
                let qname = format!("{class}.{method}");
                if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
                    return sig.returns.first().cloned();
                }
                None
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                let class = self.receiver_class(&obj.value)?;
                lookup_field_type(&class, name)
            }
            Expr::Ident(name) => self.lookup_local(name).map(|l| l.ty.clone()),
            Expr::Self_ => self
                .enclosing_class
                .as_ref()
                .map(|c| Type::Named(c.clone())),
            Expr::Str(_) => Some(Type::Named("string".into())),
            Expr::Int(_) => Some(Type::Named("integer".into())),
            Expr::Float(_) => Some(Type::Named("float".into())),
            Expr::Bool(_) => Some(Type::Named("boolean".into())),
            _ => None,
        }
    }

    fn visit_module(&mut self, m: &Module) {
        for s in &m.stmts {
            self.visit_stmt(s);
        }
    }

    fn visit_block(&mut self, b: &[Spanned<Stmt>]) {
        for s in b {
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
                let resolved = ty
                    .clone()
                    .or_else(|| value.as_ref().and_then(|v| self.infer_init_type(&v.value)))
                    .unwrap_or_else(|| Type::Named("any".into()));
                self.locals.push(LocalVar {
                    name: name.clone(),
                    ty: resolved,
                    kind: LocalKind::Local,
                });
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.visit_expr(v);
                }
                for (i, (name, ty)) in names.iter().enumerate() {
                    let resolved = ty
                        .clone()
                        .or_else(|| {
                            values
                                .get(i)
                                .and_then(|v| self.infer_init_type(&v.value))
                        })
                        .unwrap_or_else(|| Type::Named("any".into()));
                    self.locals.push(LocalVar {
                        name: name.clone(),
                        ty: resolved,
                        kind: LocalKind::Local,
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
                let mark = self.locals.len();
                self.visit_block(then_block);
                self.locals.truncate(mark);
                for (c, b) in elseifs {
                    self.visit_expr(c);
                    let mark = self.locals.len();
                    self.visit_block(b);
                    self.locals.truncate(mark);
                }
                if let Some(eb) = else_block {
                    let mark = self.locals.len();
                    self.visit_block(eb);
                    self.locals.truncate(mark);
                }
            }
            Stmt::While { cond, body } => {
                self.visit_expr(cond);
                let mark = self.locals.len();
                self.visit_block(body);
                self.locals.truncate(mark);
            }
            Stmt::Repeat { body, cond } => {
                let mark = self.locals.len();
                self.visit_block(body);
                self.visit_expr(cond);
                self.locals.truncate(mark);
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
                let mark = self.locals.len();
                self.locals.push(LocalVar {
                    name: var.clone(),
                    ty: var_ty.clone().unwrap_or_else(|| Type::Named("integer".into())),
                    kind: LocalKind::LoopVar,
                });
                self.visit_block(body);
                self.locals.truncate(mark);
            }
            Stmt::ForIn { vars, iter, body } => {
                self.visit_expr(iter);
                let mark = self.locals.len();
                for (name, ty) in vars {
                    self.locals.push(LocalVar {
                        name: name.clone(),
                        ty: ty.clone().unwrap_or_else(|| Type::Named("any".into())),
                        kind: LocalKind::LoopVar,
                    });
                }
                self.visit_block(body);
                self.locals.truncate(mark);
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
                let mark = self.locals.len();
                self.visit_block(body);
                self.locals.truncate(mark);
                let mark = self.locals.len();
                self.locals.push(LocalVar {
                    name: catch_var.clone(),
                    ty: catch_ty.clone(),
                    kind: LocalKind::Catch,
                });
                self.visit_block(catch_body);
                self.locals.truncate(mark);
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn visit_decl(&mut self, d: &Spanned<Decl>) {
        // Skip the whole subtree when the declaration's span doesn't
        // even contain the cursor — saves an analysis on every
        // unrelated top-level item in a long file.
        if !contains(&d.span, self.offset) {
            return;
        }
        match &d.value {
            Decl::Function {
                name,
                type_params,
                params,
                return_ty,
                body,
                ..
            } => {
                self.record(
                    d.span.clone(),
                    render_function_sig(name, type_params, params, return_ty.as_ref()),
                );
                for p in params {
                    self.record(p.span.clone(), render_param(p));
                    if let Some(def) = &p.default {
                        self.visit_expr(def);
                    }
                }
                let params = params.clone();
                self.enter_function(&params, |this| this.visit_block(body));
            }
            Decl::Class {
                name,
                extends,
                implements,
                members,
                ..
            } => {
                // Prefer the registry view (uniform with how `Ident`
                // hover renders the same class), falling back to the
                // raw AST head when the registry is empty — e.g. when
                // hover is invoked on a file whose semantic pass
                // hasn't run yet.
                let md = with_classes(|r| r.get(name).cloned())
                    .map(|info| render_class_full(name, &info))
                    .unwrap_or_else(|| render_class_head(name, extends.as_deref(), implements));
                self.record(d.span.clone(), md);
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
                self.record(d.span.clone(), render_interface_head(name, extends, methods));
            }
            Decl::Enum {
                name,
                variants,
                methods,
                ..
            } => {
                self.record(d.span.clone(), render_enum_head(name, variants));
                let prev = self.enclosing_class.replace(name.clone());
                for m in methods {
                    self.visit_method(m, name);
                }
                self.enclosing_class = prev;
            }
            Decl::Import { .. } => {
                // Best match wins: walk the precomputed blurbs and
                // record any whose span contains the cursor. Spans
                // come from the `Spanned<Decl>` itself, so they cover
                // the full statement.
                for (span, md) in &self.imports.import_blurbs {
                    if contains(span, self.offset) {
                        self.record(span.clone(), md.clone());
                    }
                }
            }
        }
    }

    fn visit_member(&mut self, m: &Spanned<ClassMember>) {
        if !contains(&m.span, self.offset) {
            return;
        }
        match &m.value {
            ClassMember::Field {
                is_static,
                is_private,
                name,
                ty,
                default,
            } => {
                let owner = self.enclosing_class.as_deref().unwrap_or("");
                self.record(
                    m.span.clone(),
                    render_field(owner, *is_static, *is_private, name, ty),
                );
                if let Some(def) = default {
                    self.visit_expr(def);
                }
            }
            ClassMember::Method(meth) => {
                let owner = self.enclosing_class.clone().unwrap_or_default();
                self.visit_method(meth, &owner);
            }
        }
    }

    fn visit_method(&mut self, m: &Method, owner: &str) {
        if !contains(&m.span, self.offset) {
            return;
        }
        self.record(m.span.clone(), render_method_head(owner, m));
        for p in &m.params {
            self.record(p.span.clone(), render_param(p));
            if let Some(def) = &p.default {
                self.visit_expr(def);
            }
        }
        let params = m.params.clone();
        self.enter_function(&params, |this| this.visit_block(&m.body));
    }

    fn visit_expr(&mut self, e: &Spanned<Expr>) {
        if !contains(&e.span, self.offset) {
            return;
        }
        // Record whatever this node resolves to *before* recursing, so
        // narrower children get a chance to shadow this one.
        if let Some(md) = self.expr_md(&e.value) {
            self.record(e.span.clone(), md);
        }
        match &e.value {
            Expr::Unary { rhs, .. } => self.visit_expr(rhs),
            Expr::Binary { lhs, rhs, .. } => {
                self.visit_expr(lhs);
                self.visit_expr(rhs);
            }
            Expr::Member { obj, .. } | Expr::SafeMember { obj, .. } => self.visit_expr(obj),
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
            Expr::MethodCall { obj, args, .. } => {
                self.visit_expr(obj);
                for a in args {
                    self.visit_call_arg(a);
                }
            }
            Expr::ForceUnwrap(inner) => self.visit_expr(inner),
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        saule_ast::TableEntry::Positional(v) => self.visit_expr(v),
                        saule_ast::TableEntry::Field { key, value } => {
                            self.visit_expr(key);
                            self.visit_expr(value);
                        }
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                for p in params {
                    self.record(p.span.clone(), render_param(p));
                    if let Some(def) = &p.default {
                        self.visit_expr(def);
                    }
                }
                let params = params.clone();
                self.enter_function(&params, |this| match body {
                    saule_ast::LambdaBody::Expr(b) => this.visit_expr(b),
                    saule_ast::LambdaBody::Block(b) => this.visit_block(b),
                });
            }
            Expr::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                let scrut_ty = self.infer_init_type(&scrutinee.value);
                for arm in arms {
                    let mark = self.locals.len();
                    // Bind first so the recursive `visit_pattern`
                    // walk can render `Bind` names through the
                    // local-scope path with their inferred type.
                    self.bind_pattern(&arm.pattern.value, scrut_ty.as_ref());
                    self.visit_pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    match &arm.body {
                        saule_ast::MatchBody::Expr(e) => self.visit_expr(e),
                        saule_ast::MatchBody::Block(b) => self.visit_block(b),
                    }
                    self.locals.truncate(mark);
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
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::Str(_)
            | Expr::Nil
            | Expr::Ident(_)
            | Expr::Self_ => {}
        }
    }

    fn visit_call_arg(&mut self, a: &saule_ast::CallArg) {
        match a {
            saule_ast::CallArg::Positional(e) => self.visit_expr(e),
            saule_ast::CallArg::Named { value, .. } => self.visit_expr(value),
        }
    }

    /// Walk a `match` pattern, recording hover info for the parts that
    /// have something useful to say:
    ///
    /// * `Variant { enum_name, variant, fields }` — render the variant
    ///   shape (`(variant) Enum.Variant(field: T, ...)`) and recurse
    ///   into the sub-patterns.
    /// * `Tuple(parts)` — recurse only.
    /// * `Bind(name)` — no hover here; the binding is rendered through
    ///   the local-scope path once it's been pushed by `bind_pattern`.
    /// * Literal patterns — no hover (matches today's behaviour for
    ///   literal expressions).
    fn visit_pattern(&mut self, p: &Spanned<Pattern>) {
        if !contains(&p.span, self.offset) {
            return;
        }
        match &p.value {
            Pattern::Variant {
                enum_name,
                variant,
                fields,
            } => {
                self.record(
                    p.span.clone(),
                    render_variant_pattern(
                        enum_name,
                        variant,
                        fields,
                        &self.enum_variant_fields,
                    ),
                );
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
                // Look up the just-pushed binding so the hover shows
                // its inferred type (`(binding) task: Task?`, etc.).
                if let Some(local) = self.lookup_local(name) {
                    self.record(
                        p.span.clone(),
                        format!(
                            "```saule\n(binding) {name}: {ty}\n```",
                            ty = render_type(&local.ty)
                        ),
                    );
                } else {
                    self.record(
                        p.span.clone(),
                        format!("```saule\n(binding) {name}\n```"),
                    );
                }
            }
            Pattern::Wildcard => {
                self.record(p.span.clone(), "```saule\n(wildcard) _\n```".to_string());
            }
            Pattern::Nil => {
                self.record(p.span.clone(), "```saule\n(pattern) nil\n```".to_string());
            }
            Pattern::Int(_) | Pattern::Float(_) | Pattern::Bool(_) | Pattern::Str(_) => {}
        }
    }

    /// Push every name introduced by `pat` onto the local scope, using
    /// `scrut_ty` to type top-level `Bind` and tuple bindings. Variant
    /// payload bindings are typed from the enum's recorded field
    /// types. Anything we can't type defaults to `any`.
    fn bind_pattern(&mut self, pat: &Pattern, scrut_ty: Option<&Type>) {
        match pat {
            Pattern::Bind(name) => {
                // Strip the nullable wrapper: `case nil` is the only
                // arm that handles nil, so any other arm — including
                // a bare `case binding` — implies the value is
                // non-nil. Mirrors `saule-typeck`'s arm-binding rule
                // so hover types match diagnostics.
                let ty = scrut_ty
                    .map(|t| strip_nullable_type(t.clone()))
                    .unwrap_or_else(|| Type::Named("any".into()));
                self.locals.push(LocalVar {
                    name: name.clone(),
                    ty,
                    kind: LocalKind::Binding,
                });
            }
            Pattern::Variant {
                enum_name,
                variant,
                fields,
            } => {
                let field_tys: Vec<Type> = self
                    .enum_variant_fields
                    .get(&(enum_name.clone(), variant.clone()))
                    .map(|ps| ps.iter().map(|p| p.ty.clone()).collect())
                    .unwrap_or_default();
                for (i, sub) in fields.iter().enumerate() {
                    let sub_ty = field_tys.get(i);
                    self.bind_pattern(&sub.value, sub_ty);
                }
            }
            Pattern::Tuple(parts) => {
                let elems: Option<&[Type]> = match scrut_ty {
                    Some(Type::Tuple(parts)) => Some(parts.as_slice()),
                    _ => None,
                };
                for (i, sub) in parts.iter().enumerate() {
                    let sub_ty = elems.and_then(|e| e.get(i));
                    self.bind_pattern(&sub.value, sub_ty);
                }
            }
            Pattern::Wildcard
            | Pattern::Nil
            | Pattern::Int(_)
            | Pattern::Float(_)
            | Pattern::Bool(_)
            | Pattern::Str(_) => {}
        }
    }

    /// Render a Markdown blurb for `expr` if we can resolve it from the
    /// registries / surrounding context. Returns `None` for literals
    /// and unknown names — callers should leave hover empty in that
    /// case rather than emit a misleading placeholder.
    fn expr_md(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Self_ => self
                .enclosing_class
                .as_ref()
                .map(|c| format!("```saule\n(self): {c}\n```")),
            Expr::Ident(name) => {
                // Locals shadow globals — same precedence rule the
                // resolver enforces. Render with a kind-specific
                // label so users can tell at a glance whether the
                // cursor is on a parameter, loop var, etc.
                if let Some(local) = self.lookup_local(name) {
                    let label = match local.kind {
                        LocalKind::Param => "(parameter)",
                        LocalKind::Local => "(local)",
                        LocalKind::LoopVar => "(loop var)",
                        LocalKind::Catch => "(error)",
                        LocalKind::Binding => "(binding)",
                    };
                    return Some(format!(
                        "```saule\n{label} {name}: {ty}\n```",
                        ty = render_type(&local.ty)
                    ));
                }
                self.resolve_ident(name)
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                let class = self.receiver_class(&obj.value)?;
                resolve_member(&class, name, false)
            }
            Expr::MethodCall { obj, method, .. } => {
                let class = self.receiver_class(&obj.value)?;
                resolve_member(&class, method, true)
            }
            _ => None,
        }
    }

    /// Resolve a bare identifier to a hover blurb. Tries class /
    /// interface / enum registries (which include builtins and
    /// seed-imported classes), then falls back to the native-signature
    /// registry for stdlib free functions and modules, then finally to
    /// the per-request import context for top-level functions imported
    /// from another `.sau` file. Returns `None` for names we can't tie
    /// to anything (locals, parameters, unknown idents).
    fn resolve_ident(&self, name: &str) -> Option<String> {
        if let Some(info) = with_classes(|r| r.get(name).cloned()) {
            return Some(render_class_full(name, &info));
        }
        if with_interfaces(|r| r.contains_key(name)) {
            let extends = with_interfaces(|r| r.get(name).cloned()).unwrap_or_default();
            return Some(render_interface_from_registry(name, &extends));
        }
        if with_enums(|r| r.contains_key(name)) {
            let info = with_enums(|r| r.get(name).cloned())?;
            let variants: Vec<(String, usize)> =
                info.variants.iter().map(|(n, a)| (n.clone(), *a)).collect();
            return Some(render_enum_from_registry(name, &variants));
        }
        if let Some(sig) = saule_typeck::sigs::lookup(name) {
            return Some(format!(
                "```saule\nfn {name}{}\n```",
                render_native_sig_full(&sig)
            ));
        }
        if saule_typeck::sigs::is_value_type(name) {
            return Some(render_stdlib_module(name, "type"));
        }
        if saule_typeck::sigs::is_module(name) {
            return Some(render_stdlib_module(name, "module"));
        }
        // Imported user function — final fallback. The caller built
        // this map from the current module's `import` declarations.
        if let Some(md) = self.imports.fn_sigs.get(name) {
            return Some(md.clone());
        }
        None
    }

    /// Best-effort: figure out which class a member-access receiver
    /// refers to. Handles `self`, bare class-name references (static
    /// access like `Math.sqrt`), and chained `Class.foo.bar` where the
    /// inner field's declared type is a named class.
    fn receiver_class(&self, obj: &Expr) -> Option<String> {
        match obj {
            Expr::Self_ => self.enclosing_class.clone(),
            Expr::Ident(name) => {
                // Locals first — `newEntry.setDone(...)` resolves
                // through the local's declared/inferred type.
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
                // `Class(args).foo` — constructor call returns the class.
                if let Expr::Ident(name) = &callee.value
                    && with_classes(|r| r.contains_key(name))
                {
                    return Some(name.clone());
                }
                None
            }
            Expr::MethodCall { obj: inner, method, .. } => {
                // `obj:method(args).foo` — chase the method's
                // registered return type.
                let inner_class = self.receiver_class(&inner.value)?;
                let sig = lookup_method(&inner_class, method)?;
                named_type(sig.return_ty.as_ref()?)
            }
            _ => None,
        }
    }
}

