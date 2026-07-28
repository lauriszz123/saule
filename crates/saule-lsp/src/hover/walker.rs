//! AST walker that picks the smallest enclosing node at a byte offset
//! and produces a Markdown blurb for it. The big stateful machinery
//! (`Cx`, scope tracking, every `visit_*` arm) lives here; rendering
//! helpers live in [`super::render`] and shared utilities in
//! [`super::util`].

use std::collections::HashMap;
use std::ops::Range;

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, Method, Module, Param, Pattern, Spanned, Stmt, Type,
};
use saule_semantic::{
    lookup_field_type, lookup_method, super_init_target, with_classes, with_enums, with_interfaces,
};

use super::ImportContext;
use super::render::{
    collect_enum_variant_fields, render_class_full, render_class_head, render_enum_from_registry,
    render_enum_head, render_enum_variant_decl, render_field, render_function_sig,
    render_interface_from_registry, render_interface_head, render_interface_method,
    render_method_head, render_method_sig, render_native_sig_full, render_param,
    render_stdlib_module, render_type, render_variant_pattern, with_doc, with_param_doc,
};
use super::util::{
    collect_named_heads, contains, is_primitive, locate_word_in, named_type, resolve_member,
    strip_nullable_type,
};

/// Collapse an inferred type to the single value it yields in a
/// single-value context: a multi-return tuple becomes its first
/// component, anything else passes through, and `None` becomes `any`.
fn first_value_type(ty: Option<Type>) -> Type {
    match ty {
        Some(Type::Tuple(parts)) => parts
            .into_iter()
            .next()
            .unwrap_or_else(|| Type::Named("any".into())),
        Some(t) => t,
        None => Type::Named("any".into()),
    }
}

/// Drive the hover walker against `module` for `offset` and return the
/// best (smallest-span) blurb we found, if any.
pub(super) fn run(
    module: &Module,
    source: &str,
    offset: usize,
    imports: &ImportContext,
) -> Option<(String, Range<usize>)> {
    let mut cx = Cx {
        source,
        offset,
        enclosing_class: None,
        enclosing_return_ty: None,
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
    source: &'a str,
    offset: usize,
    enclosing_class: Option<String>,
    /// Declared return type of the innermost enclosing
    /// function/method. `None` when outside any function or when the
    /// function omitted its `-> T` annotation. Used to surface
    /// `return` keyword hover.
    enclosing_return_ty: Option<Type>,
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

    /// The `---` doc comment attached to the declaration starting at
    /// `anchor`, or `None` when it has none (or an empty one).
    ///
    /// Callers reach for this once per declaration and thread the
    /// result through to the parameter loop, so a hover on a parameter
    /// can surface its `@param` line without re-scanning the source.
    fn doc_at(&self, anchor: usize) -> Option<saule_docs::DocBlock> {
        saule_docs::extract(self.source, anchor).filter(|d| !d.is_empty())
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

    /// Like [`enter_function`] but also tracks the function's declared
    /// return type so a hover on the `return` keyword inside the body
    /// can surface it.
    fn enter_function_with_return(
        &mut self,
        params: &[Param],
        return_ty: Option<&Type>,
        body: impl FnOnce(&mut Self),
    ) {
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_ret = self.enclosing_return_ty.take();
        self.enclosing_return_ty = return_ty.cloned();
        for p in params {
            self.locals.push(LocalVar {
                name: p.name.clone(),
                ty: p.ty.clone(),
                kind: LocalKind::Param,
            });
        }
        body(self);
        self.locals = saved_locals;
        self.enclosing_return_ty = saved_ret;
    }

    /// Surface hover info for every named-type head reachable from
    /// `ty` (including nullable / table / tuple / function components)
    /// that appears as an identifier inside `search_span` of the
    /// source. Used to make type ascriptions on params, class fields,
    /// return types, etc. resolve the same way bare identifier hovers
    /// do — without baking individual `ty_span` fields into every AST
    /// node that carries a `Type`.
    fn record_type_idents_in(&mut self, ty: &Type, search_span: &Range<usize>) {
        let mut names: Vec<String> = Vec::new();
        collect_named_heads(ty, &mut names);
        for name in names {
            // Skip primitives — they have no useful hover and would
            // mask the parent expression's blurb otherwise.
            if is_primitive(&name) {
                continue;
            }
            if let Some(span) = locate_word_in(self.source, search_span, &name) {
                if !contains(&span, self.offset) {
                    continue;
                }
                if let Some(md) = self.resolve_ident(&name) {
                    self.record(span, md);
                }
            }
        }
    }

    /// Like [`record_type_idents_in`] but takes a list of bare names
    /// (e.g. the parents listed after `extends` / `implements`) and
    /// finds each one inside `search_span`. Names may legitimately
    /// repeat, so we use the first occurrence — that's good enough
    /// for the cases we care about (class headers don't reference the
    /// same parent twice).
    fn record_named_idents_in(&mut self, names: &[String], search_span: &Range<usize>) {
        for name in names {
            if let Some(span) = locate_word_in(self.source, search_span, name) {
                if !contains(&span, self.offset) {
                    continue;
                }
                if let Some(md) = self.resolve_ident(name) {
                    self.record(span, md);
                }
            }
        }
    }

    /// Look up `name` in the current local scope (innermost first).
    /// Returns `None` for free identifiers — the caller falls through
    /// to the registry / native-sig path.
    fn lookup_local(&self, name: &str) -> Option<&LocalVar> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

    /// Infer the types of a call's positional arguments (in order;
    /// `None` where inference can't produce a type). Named arguments are
    /// skipped — mirrors how the typechecker binds generics from
    /// positional args only.
    fn positional_arg_types(&self, args: &[CallArg]) -> Vec<Option<Type>> {
        args.iter()
            .filter_map(|a| match a {
                CallArg::Positional(e) => Some(self.infer_init_type(&e.value)),
                CallArg::Named { .. } => None,
            })
            .collect()
    }

    /// Expand a value-expression list into the flat list of value types it
    /// produces, using Lua-style multi-assign semantics (mirrors the
    /// interpreter's `eval_expr_list`): every expression contributes exactly
    /// one value *except the last*, whose tuple components (a multi-return)
    /// spread into several. Non-final expressions are in single-value context,
    /// so a tuple there collapses to its first component.
    fn spread_value_types(&self, values: &[Spanned<Expr>]) -> Vec<Type> {
        let mut out = Vec::new();
        let n = values.len();
        for (i, v) in values.iter().enumerate() {
            let ty = self.infer_init_type(&v.value);
            if i + 1 == n {
                match ty {
                    Some(Type::Tuple(parts)) => out.extend(parts),
                    other => out.push(first_value_type(other)),
                }
            } else {
                out.push(first_value_type(ty));
            }
        }
        out
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
            // `x as T` is always `T?` — the cast can fail, and the
            // nullable result is what forces the caller to handle it.
            Expr::Cast { ty, .. } => Some(Type::Nullable(Box::new(ty.clone()))),
            Expr::Call { callee, args } => {
                if let Expr::Ident(name) = &callee.value {
                    if with_classes(|r| r.contains_key(name)) {
                        return Some(Type::Named(name.clone()));
                    }
                    // Non-constructor free call: consult native-sig
                    // returns or imported function signatures. We
                    // don't have ASTs for those, so return None and
                    // accept `any`.
                    if let Some(sig) = saule_typeck::sigs::lookup(name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                            .into_iter()
                            .next();
                    }
                    // Sibling free function inside a class body —
                    // reach through the enclosing-class registry the
                    // same way `resolve_ident` does for hover.
                    if let Some(class) = &self.enclosing_class {
                        if let Some(sig) = lookup_method(class, name) {
                            let arg_types = self.positional_arg_types(args);
                            return saule_typeck::sigs::instantiate_method_return(&sig, &arg_types);
                        }
                    }
                }
                // `recv.method(args)` — dot-call on an instance or
                // module. Resolve the receiver's class and chase the
                // method's return type the same way `MethodCall` does.
                if let Expr::Member { obj, name } = &callee.value {
                    let class = self.receiver_class(&obj.value)?;
                    if let Some(sig) = lookup_method(&class, name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_method_return(&sig, &arg_types);
                    }
                    let qname = format!("{class}.{name}");
                    if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                            .into_iter()
                            .next();
                    }
                }
                None
            }
            Expr::MethodCall { obj, method, args } => {
                let class = self.receiver_class(&obj.value)?;
                if let Some(sig) = lookup_method(&class, method) {
                    let arg_types = self.positional_arg_types(args);
                    return saule_typeck::sigs::instantiate_method_return(&sig, &arg_types);
                }
                let qname = format!("{class}.{method}");
                if let Some(sig) = saule_typeck::sigs::lookup(&qname) {
                    let arg_types = self.positional_arg_types(args);
                    return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                        .into_iter()
                        .next();
                }
                None
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                let class = self.receiver_class(&obj.value)?;
                lookup_field_type(&class, name)
            }
            Expr::Index { obj, .. } => {
                // Element type of `table<V>` / `table<K, V>`. Anything
                // else (e.g. indexing a class instance) we don't try
                // to resolve here — typeck handles that statically.
                let obj_ty = self.infer_init_type(&obj.value)?;
                match obj_ty {
                    Type::Table { value, .. } => Some(*value),
                    Type::Nullable(inner) => match *inner {
                        Type::Table { value, .. } => Some(*value),
                        _ => None,
                    },
                    _ => None,
                }
            }
            Expr::ForceUnwrap(inner) => {
                let ty = self.infer_init_type(&inner.value)?;
                Some(strip_nullable_type(ty))
            }
            Expr::Unary { op, rhs } => match op {
                saule_ast::UnaryOp::Not => Some(Type::Named("boolean".into())),
                saule_ast::UnaryOp::Len => Some(Type::Named("integer".into())),
                saule_ast::UnaryOp::Neg => self.infer_init_type(&rhs.value),
            },
            Expr::Binary { op, lhs, rhs } => {
                use saule_ast::BinOp::*;
                match op {
                    Eq | NotEq | Lt | LtEq | Gt | GtEq | And | Or => {
                        Some(Type::Named("boolean".into()))
                    }
                    Concat => Some(Type::Named("string".into())),
                    Coalesce => self
                        .infer_init_type(&lhs.value)
                        .map(strip_nullable_type)
                        .or_else(|| self.infer_init_type(&rhs.value)),
                    Add | Sub | Mul | Div | Mod => self
                        .infer_init_type(&lhs.value)
                        .or_else(|| self.infer_init_type(&rhs.value)),
                }
            }
            Expr::Lambda {
                params, return_ty, ..
            } => Some(Type::Function {
                params: params.iter().map(|p| p.ty.clone()).collect(),
                ret: Box::new(
                    return_ty
                        .clone()
                        .unwrap_or_else(|| Type::Named("any".into())),
                ),
            }),
            Expr::Table(entries) => Some(self.infer_table_literal(entries)),
            Expr::Match { arms, .. } => {
                // First arm's body type is a decent approximation;
                // typeck enforces that all arms agree.
                arms.first().and_then(|arm| match &arm.body {
                    saule_ast::MatchBody::Expr(e) => self.infer_init_type(&e.value),
                    saule_ast::MatchBody::Block(b) => match b.last().map(|s| &s.value) {
                        Some(Stmt::Expr(e)) => self.infer_init_type(&e.value),
                        Some(Stmt::Return(rs)) => {
                            rs.first().and_then(|e| self.infer_init_type(&e.value))
                        }
                        _ => None,
                    },
                })
            }
            Expr::Pipe { stages, .. } => {
                // Last stage's return type. Each stage is `:fn(args)`
                // where `fn` is a free function; the piped value is
                // prepended at call time.
                let last = stages.last()?;
                if let Some(sig) = saule_typeck::sigs::lookup(&last.name) {
                    return sig.returns.first().cloned();
                }
                None
            }
            Expr::Ident(name) => {
                if let Some(local) = self.lookup_local(name) {
                    return Some(local.ty.clone());
                }
                // Bare class / module / value-type name — its "value"
                // type is the named class itself. Useful for `let s =
                // Storage` (no call) and indirectly for method-chain
                // inference upstream.
                if with_classes(|r| r.contains_key(name))
                    || ((saule_typeck::sigs::is_module(name)
                        || saule_typeck::sigs::is_value_type(name))
                        && saule_semantic::prelude::contains(name))
                {
                    return Some(Type::Named(name.clone()));
                }
                None
            }
            Expr::Self_ => self
                .enclosing_class
                .as_ref()
                .map(|c| Type::Named(c.clone())),
            Expr::Str(_) => Some(Type::Named("string".into())),
            Expr::Int(_) => Some(Type::Named("integer".into())),
            Expr::Float(_) => Some(Type::Named("float".into())),
            Expr::Bool(_) => Some(Type::Named("boolean".into())),
            Expr::Nil => Some(Type::Nullable(Box::new(Type::Named("any".into())))),
        }
    }

    /// Infer a `table<V>` (array literal) or `table<K, V>` (map literal)
    /// from a table-constructor's entries, so generic natives like
    /// `Util.map(table<T>, …)` can bind their element type. Falls back to a
    /// bare `table` when the entries are empty or their types disagree.
    fn infer_table_literal(&self, entries: &[saule_ast::TableEntry]) -> Type {
        use saule_ast::TableEntry;
        let mut value_ty: Option<Type> = None;
        let mut key_ty: Option<Type> = None;
        let mut has_field = false;
        let mut consistent = true;
        for entry in entries {
            let (k, v) = match entry {
                TableEntry::Positional(v) => (None, self.infer_init_type(&v.value)),
                TableEntry::Field { key, value } => {
                    has_field = true;
                    (
                        self.infer_init_type(&key.value),
                        self.infer_init_type(&value.value),
                    )
                }
            };
            if let Some(vt) = v {
                match &value_ty {
                    Some(existing) if existing != &vt => consistent = false,
                    _ => value_ty = Some(vt),
                }
            }
            if let Some(kt) = k {
                match &key_ty {
                    Some(existing) if existing != &kt => consistent = false,
                    _ => key_ty = Some(kt),
                }
            }
        }
        match value_ty {
            Some(v) if consistent => Type::Table {
                key: if has_field {
                    key_ty.map(Box::new)
                } else {
                    None
                },
                value: Box::new(v),
            },
            _ => Type::Named("table".into()),
        }
    }

    /// Refine a bare structural annotation (`table` / `function`) against the
    /// initializer's inferred shape: `local nums: table = {1, 2}` becomes
    /// `table<integer>`. A `nil`-side or mismatched-kind value leaves the
    /// declared type untouched. Mirrors typeck's `refine_bare_binding`.
    fn refine_bare_annotation(&self, decl: Type, value: Option<Type>) -> Type {
        let Type::Named(name) = &decl else {
            return decl;
        };
        let Some(value_ty) = value else {
            return decl;
        };
        let inner = match &value_ty {
            Type::Nullable(i) => (**i).clone(),
            other => other.clone(),
        };
        let matches_kind = matches!(
            (name.as_str(), &inner),
            ("table", Type::Table { .. }) | ("function", Type::Function { .. })
        );
        if matches_kind { inner } else { decl }
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
            Stmt::Local {
                name,
                name_span,
                ty,
                ty_span,
                value,
            } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                let resolved = match ty.clone() {
                    // A bare structural annotation (`table` / `function`)
                    // is refined against the initializer's inferred shape
                    // so generic natives can bind the element type
                    // (`local nums: table = {1, 2}` -> `table<integer>`).
                    Some(t) => self.refine_bare_annotation(
                        t,
                        value.as_ref().and_then(|v| self.infer_init_type(&v.value)),
                    ),
                    // A single binding takes one value; a multi-return
                    // (tuple) collapses to its first component here.
                    None => first_value_type(
                        value.as_ref().and_then(|v| self.infer_init_type(&v.value)),
                    ),
                };
                // Surface a hover blurb when the cursor is on the
                // declaring identifier itself (`local due: int? = …`
                // -> hover on `due`). Without this the walker would
                // only fire on later *uses* of the name.
                self.record(
                    name_span.clone(),
                    format!(
                        "```saule\n(local) {name}: {ty}\n```",
                        ty = render_type(&resolved)
                    ),
                );
                // Cursor on the type ascription itself
                // (`local s: Storage = …` -> hover on `Storage`):
                // resolve the head named type through the same
                // identifier path that handles bare class / interface
                // / enum references in expressions.
                if let (Some(span), Some(t)) = (ty_span, ty.as_ref()) {
                    if let Some(head) = named_type(t) {
                        if let Some(md) = self.resolve_ident(&head) {
                            self.record(span.clone(), md);
                        }
                    }
                }
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
                let spread = self.spread_value_types(values);
                for (i, (name, name_span, ty)) in names.iter().enumerate() {
                    let resolved = ty
                        .clone()
                        .or_else(|| spread.get(i).cloned())
                        .unwrap_or_else(|| Type::Named("any".into()));
                    // Surface a hover blurb when the cursor is on the
                    // declaring identifier itself (`local q, r = …`
                    // -> hover on `q`), mirroring `Stmt::Local`.
                    self.record(
                        name_span.clone(),
                        format!(
                            "```saule\n(local) {name}: {ty}\n```",
                            ty = render_type(&resolved)
                        ),
                    );
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
                    ty: var_ty
                        .clone()
                        .unwrap_or_else(|| Type::Named("integer".into())),
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
                // Hover on the `return` keyword surfaces the enclosing
                // function's declared return type. The keyword sits at
                // the start of the statement span; we use the first
                // 6 bytes (`return`) as the hit region, falling back
                // to the whole stmt span when the source is empty
                // (unit tests that don't pass a source string).
                if let Some(rt) = self.enclosing_return_ty.clone() {
                    let kw_end = (s.span.start + "return".len()).min(s.span.end);
                    let kw_span = s.span.start..kw_end;
                    self.record(
                        kw_span,
                        format!("```saule\n(return) -> {ty}\n```", ty = render_type(&rt)),
                    );
                }
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
                let doc = self.doc_at(d.span.start);
                self.record(
                    d.span.clone(),
                    with_doc(
                        render_function_sig(name, type_params, params, return_ty.as_ref()),
                        doc.as_ref(),
                    ),
                );
                for p in params {
                    self.record(
                        p.span.clone(),
                        with_param_doc(render_param(p), doc.as_ref(), &p.name),
                    );
                    // Type ascription on the param: `x: Storage` —
                    // make `Storage` itself hoverable.
                    self.record_type_idents_in(&p.ty, &p.span);
                    if let Some(def) = &p.default {
                        self.visit_expr(def);
                    }
                }
                if let Some(rt) = return_ty {
                    // Locate the return type slice between the closing
                    // `)` of the param list and the start of the body.
                    let after = params.last().map(|p| p.span.end).unwrap_or(d.span.start);
                    let before = body.first().map(|s| s.span.start).unwrap_or(d.span.end);
                    self.record_type_idents_in(rt, &(after..before));
                }
                let params = params.clone();
                let return_ty = return_ty.clone();
                self.enter_function_with_return(&params, return_ty.as_ref(), |this| {
                    this.visit_block(body)
                });
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
                let doc = self.doc_at(d.span.start);
                self.record(d.span.clone(), with_doc(md, doc.as_ref()));
                // Resolve `extends X` / `implements Y, Z` parent and
                // interface names within the class header (between
                // the class name and the first member). We use the
                // first occurrence of each name within the decl span
                // as the hover target.
                let header_end = members.first().map(|m| m.span.start).unwrap_or(d.span.end);
                let header_span = d.span.start..header_end;
                if let Some(parent) = extends {
                    self.record_named_idents_in(&[parent.clone()], &header_span);
                }
                self.record_named_idents_in(implements, &header_span);
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
                let doc = self.doc_at(d.span.start);
                self.record(
                    d.span.clone(),
                    with_doc(render_interface_head(name, extends, methods), doc.as_ref()),
                );
                // Same idea as class `extends` — locate parent
                // interface names within the interface decl span.
                self.record_named_idents_in(extends, &d.span);
                // Each bodiless method is a hover target of its own so
                // a `---` block written above it has somewhere to go.
                for m in methods {
                    if !contains(&m.span, self.offset) {
                        continue;
                    }
                    let mdoc = self.doc_at(m.span.start);
                    self.record(
                        m.span.clone(),
                        with_doc(render_interface_method(name, m), mdoc.as_ref()),
                    );
                    for p in &m.params {
                        self.record(
                            p.span.clone(),
                            with_param_doc(render_param(p), mdoc.as_ref(), &p.name),
                        );
                        self.record_type_idents_in(&p.ty, &p.span);
                    }
                }
            }
            Decl::Enum {
                name,
                variants,
                methods,
                ..
            } => {
                let doc = self.doc_at(d.span.start);
                self.record(
                    d.span.clone(),
                    with_doc(render_enum_head(name, variants), doc.as_ref()),
                );
                for v in variants {
                    if !contains(&v.span, self.offset) {
                        continue;
                    }
                    let vdoc = self.doc_at(v.span.start);
                    self.record(
                        v.span.clone(),
                        with_doc(render_enum_variant_decl(name, &v.value), vdoc.as_ref()),
                    );
                    if let saule_ast::EnumVariant::Tuple { fields, .. } = &v.value {
                        for p in fields {
                            self.record(
                                p.span.clone(),
                                with_param_doc(render_param(p), vdoc.as_ref(), &p.name),
                            );
                        }
                    }
                }
                let prev = self.enclosing_class.replace(name.clone());
                for m in methods {
                    self.visit_method(m, name);
                }
                self.enclosing_class = prev;
            }
            Decl::Import { names, .. } => {
                // Best match wins: walk the precomputed blurbs and
                // record any whose span contains the cursor. Spans
                // come from the `Spanned<Decl>` itself, so they cover
                // the full statement.
                for (span, md) in &self.imports.import_blurbs {
                    if contains(span, self.offset) {
                        self.record(span.clone(), md.clone());
                    }
                }
                // Per-name resolution: if the cursor is on one of the
                // listed import names, prefer its specific class /
                // interface / enum / function blurb over the generic
                // statement-wide one. Aliased imports (`X as Y`)
                // resolve through the alias name the user typed.
                if let saule_ast::ImportNames::List(items) = names {
                    for (orig, alias) in items {
                        let local = alias.as_deref().unwrap_or(orig);
                        // Local alias span — the name the importer
                        // sees and would hover.
                        if let Some(span) = locate_word_in(self.source, &d.span, local) {
                            if contains(&span, self.offset) {
                                if let Some(md) = self
                                    .resolve_ident(local)
                                    .or_else(|| self.imports.fn_sigs.get(local).cloned())
                                {
                                    self.record(span, md);
                                }
                            }
                        }
                        // Original (upstream) name when an alias was
                        // used: `import X as Y` — both should hover.
                        if alias.is_some() {
                            if let Some(span) = locate_word_in(self.source, &d.span, orig) {
                                if contains(&span, self.offset) {
                                    if let Some(md) = self.resolve_ident(orig) {
                                        self.record(span, md);
                                    }
                                }
                            }
                        }
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
                let doc = self.doc_at(m.span.start);
                self.record(
                    m.span.clone(),
                    with_doc(
                        render_field(owner, *is_static, *is_private, name, ty),
                        doc.as_ref(),
                    ),
                );
                // Make `due: integer?` -> hover on `integer` resolve
                // through the same path bare ident hovers use.
                self.record_type_idents_in(ty, &m.span);
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
        let doc = self.doc_at(m.span.start);
        self.record(
            m.span.clone(),
            with_doc(render_method_head(owner, m), doc.as_ref()),
        );
        for p in &m.params {
            self.record(
                p.span.clone(),
                with_param_doc(render_param(p), doc.as_ref(), &p.name),
            );
            self.record_type_idents_in(&p.ty, &p.span);
            if let Some(def) = &p.default {
                self.visit_expr(def);
            }
        }
        if let Some(rt) = &m.return_ty {
            let after = m.params.last().map(|p| p.span.end).unwrap_or(m.span.start);
            let before = m.body.first().map(|s| s.span.start).unwrap_or(m.span.end);
            self.record_type_idents_in(rt, &(after..before));
        }
        let params = m.params.clone();
        let return_ty = m.return_ty.clone();
        self.enter_function_with_return(&params, return_ty.as_ref(), |this| {
            this.visit_block(&m.body)
        });
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
            // Recurse into the operand so the cast is transparent for
            // hover; the target type's own idents are handled below.
            Expr::Cast { value, ty } => {
                self.visit_expr(value);
                self.record_type_idents_in(ty, &e.span);
            }
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
                let params = self.callee_params(&callee.value);
                for a in args {
                    self.visit_call_arg_with_params(a, params.as_deref());
                }
            }
            Expr::MethodCall { obj, method, args } => {
                self.visit_expr(obj);
                let params = self
                    .receiver_class(&obj.value)
                    .and_then(|c| lookup_method(&c, method))
                    .map(|sig| sig.params);
                for a in args {
                    self.visit_call_arg_with_params(a, params.as_deref());
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
                    // Stdlib pipe stages (`|> Math.sqrt`) carry only
                    // positional types — no parameter names — so we
                    // can't resolve named-arg keys against them.
                    for a in &st.args {
                        self.visit_call_arg_with_params(a, None);
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

    #[allow(dead_code)]
    fn visit_call_arg(&mut self, a: &saule_ast::CallArg) {
        match a {
            saule_ast::CallArg::Positional(e) => self.visit_expr(e),
            saule_ast::CallArg::Named { value, .. } => self.visit_expr(value),
        }
    }

    /// Like [`visit_call_arg`] but also surfaces hover info on a named
    /// argument's *key* (`storage.add(item, dueDate: due)` -> hovering
    /// on `dueDate` shows the parameter declaration). `params` carries
    /// the callee's parameter list when we could resolve it, used to
    /// look up the matching declared type.
    fn visit_call_arg_with_params(&mut self, a: &saule_ast::CallArg, params: Option<&[Param]>) {
        match a {
            saule_ast::CallArg::Positional(e) => self.visit_expr(e),
            saule_ast::CallArg::Named { name, value } => {
                let key_search = value.span.start.saturating_sub(name.len() + 4)..value.span.start;
                if let Some(span) = locate_word_in(self.source, &key_search, name) {
                    if contains(&span, self.offset) {
                        let md = match params.and_then(|ps| ps.iter().find(|p| &p.name == name)) {
                            Some(p) => format!(
                                "```saule\n(named arg) {name}: {ty}\n```",
                                ty = render_type(&p.ty)
                            ),
                            None => format!("```saule\n(named arg) {name}\n```"),
                        };
                        self.record(span, md);
                    }
                }
                self.visit_expr(value);
            }
        }
    }

    /// Best-effort: extract the declared parameter list of `callee`
    /// for named-argument hover. Handles the common shapes —
    /// constructor calls, sibling free / static calls, and free
    /// function references — without trying to be exhaustive.
    fn callee_params(&self, callee: &Expr) -> Option<Vec<Param>> {
        match callee {
            Expr::Ident(name) => {
                // Constructor: `init` method, falling through to a
                // bare `Class()` call (which uses no init params).
                if with_classes(|r| r.contains_key(name)) {
                    return lookup_method(name, "init").map(|sig| sig.params);
                }
                if let Some(class) = &self.enclosing_class {
                    if let Some(sig) = lookup_method(class, name) {
                        return Some(sig.params);
                    }
                }
                if let Some(sig) = saule_typeck::sigs::lookup(name) {
                    let _ = sig;
                    // Native sigs only know positional types, not
                    // parameter names — they can't drive named-arg
                    // hover. Treat them as unresolved.
                    return None;
                }
                None
            }
            Expr::Member { obj, name } => {
                if let Some((_, sig)) = self.super_target(name, &obj.value) {
                    return Some(sig.params);
                }
                let class = self.receiver_class(&obj.value)?;
                if let Some(sig) = lookup_method(&class, name) {
                    return Some(sig.params);
                }
                None
            }
            _ => None,
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
                    render_variant_pattern(enum_name, variant, fields, &self.enum_variant_fields),
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
                    self.record(p.span.clone(), format!("```saule\n(binding) {name}\n```"));
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
                // `self.super(...)` isn't a member access — surface the
                // parent constructor it delegates to instead.
                if let Some((owner, sig)) = self.super_target(name, &obj.value) {
                    let enclosing = self.enclosing_class.clone().unwrap_or_default();
                    return Some(format!(
                        "{sig}\nParent constructor, delegated to by `self.super(...)` in `{enclosing}`.",
                        sig = render_method_sig(&owner, "init", &sig)
                    ));
                }
                let class = self.receiver_class(&obj.value)?;
                resolve_member(&class, name, false, &self.imports.docs)
            }
            Expr::MethodCall { obj, method, .. } => {
                let class = self.receiver_class(&obj.value)?;
                resolve_member(&class, method, true, &self.imports.docs)
            }
            // Generic fallback: surface the inferred type for any
            // expression we can reason about (`#args` -> integer,
            // `args[1]` -> string, `not foo` -> boolean, lambdas,
            // call results, etc.). Without this, hovering on the
            // gaps between named children returned `None`.
            other => self
                .infer_init_type(other)
                .map(|ty| format!("```saule\n(expr): {ty}\n```", ty = render_type(&ty))),
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
        // Type declarations resolve through the registries, which hold
        // no source text — pull any `---` block from the doc index so a
        // hover on a *usage* reads the same as one on the declaration.
        let doc = self.imports.docs.get(name);
        if let Some(info) = with_classes(|r| r.get(name).cloned()) {
            return Some(with_doc(render_class_full(name, &info), doc));
        }
        if with_interfaces(|r| r.contains_key(name)) {
            let extends = with_interfaces(|r| r.get(name).cloned()).unwrap_or_default();
            return Some(with_doc(
                render_interface_from_registry(name, &extends),
                doc,
            ));
        }
        if with_enums(|r| r.contains_key(name)) {
            let info = with_enums(|r| r.get(name).cloned())?;
            let variants: Vec<(String, usize)> =
                info.variants.iter().map(|(n, a)| (n.clone(), *a)).collect();
            return Some(with_doc(render_enum_from_registry(name, &variants), doc));
        }
        if let Some(sig) = saule_typeck::sigs::lookup(name) {
            return Some(format!(
                "```saule\nfn {name}{}\n```",
                render_native_sig_full(&sig)
            ));
        }
        if saule_typeck::sigs::is_value_type(name) && saule_semantic::prelude::contains(name) {
            return Some(render_stdlib_module(name, "type"));
        }
        if saule_typeck::sigs::is_module(name) && saule_semantic::prelude::contains(name) {
            return Some(render_stdlib_module(name, "module"));
        }
        // Bare identifier inside a class body — try resolving as a
        // method or field of the enclosing class. Covers calling a
        // sibling static (`help()` from inside `Main.main()`) and
        // referencing a sibling instance member without `self.`.
        if let Some(class) = &self.enclosing_class {
            if let Some(md) = resolve_member(class, name, false, &self.imports.docs) {
                return Some(md);
            }
        }
        // Imported user function — final fallback. The caller built
        // this map from the current module's `import` declarations.
        if let Some(md) = self.imports.fn_sigs.get(name) {
            return Some(md.clone());
        }
        None
    }

    /// `(owner, sig)` of the constructor a `self.super(...)` written in
    /// the current class delegates to, when `name` / `obj` are exactly
    /// that form. `None` for every other member access.
    fn super_target(&self, name: &str, obj: &Expr) -> Option<(String, saule_semantic::MethodSig)> {
        if name != "super" || !matches!(obj, Expr::Self_) {
            return None;
        }
        super_init_target(self.enclosing_class.as_deref()?)
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
                    || ((saule_typeck::sigs::is_module(name)
                        || saule_typeck::sigs::is_value_type(name))
                        && saule_semantic::prelude::contains(name))
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
            Expr::MethodCall {
                obj: inner, method, ..
            } => {
                // `obj:method(args).foo` — chase the method's
                // registered return type.
                let inner_class = self.receiver_class(&inner.value)?;
                let sig = lookup_method(&inner_class, method)?;
                named_type(sig.return_ty.as_ref()?)
            }
            Expr::Index { obj: inner, .. } => {
                // `tbl[i].foo` — the receiver's class is the element
                // type of the indexed `table<…>` (or the `value` half
                // of a `table<K, V>`). Walk the inner expression to
                // get its declared type, then peel one level.
                let ty = self.infer_init_type(&inner.value)?;
                match ty {
                    Type::Table { value, .. } => named_type(&value),
                    other => named_type(&other),
                }
            }
            _ => None,
        }
    }
}
