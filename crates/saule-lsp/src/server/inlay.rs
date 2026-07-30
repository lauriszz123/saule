//! Inlay hint provider — emits two kinds of hints:
//!
//! * **Type hints on locals** — `local x = Foo()` → `x : Foo` ghost
//!   text after the local's name. Only fires when the right-hand side
//!   has a confidently-inferrable named type (constructor call,
//!   string/integer/float/bool literal, identifier referencing a
//!   typed local). This deliberately stays narrow: false-positive
//!   inlay hints are visually loud and we'd rather skip than mislead.
//!
//! * **Parameter-name hints on positional call args** — `add(2, 3)` →
//!   `add(a: 2, b: 3)` ghost labels in front of each argument so the
//!   reader sees what slot they're filling. Suppressed when the arg
//!   expression already mentions the param name (`add(a, b)` shouldn't
//!   show `a: a`) and for trivially-named single-arg calls
//!   (`println("..."`)).
//!
//! Hints are produced from the same thread-local class / interface /
//! enum registries the hover walker uses, so they reflect imports
//! seeded by `analyze_with_seed`.

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, LambdaBody, MatchBody, Method, Module, Param, Spanned,
    Stmt, TableEntry, Type,
};
use saule_semantic::{lookup_method, with_classes};
use std::collections::HashMap;
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Url};

use crate::line_index::LineIndex;

use super::{canonical, Backend};

impl Backend {
    /// Compute inlay hints for `uri`. Re-runs analysis under the shared
    /// lock so the registries reflect the current document. Returns an
    /// empty vec on lex / parse failure or for closed documents.
    pub(super) async fn inlay_hints(&self, uri: &Url) -> Vec<InlayHint> {
        let entry = match self.docs.get(uri.as_str()) {
            Some(e) => e,
            None => return Vec::new(),
        };
        let source = entry.source.clone();
        drop(entry);

        let Ok(tokens) = saule_lexer::Lexer::new(&source).tokenize() else {
            return Vec::new();
        };
        let Ok(module) = saule_parser::parse(tokens) else {
            return Vec::new();
        };
        let line_index = LineIndex::new(&source);

        let module_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| canonical(&p))
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let _guard = self.analysis_lock.lock().await;
        if let Some(info) = self.project_info.lock().await.clone() {
            saule_interpreter::project::set(info);
        }
        let seed = match &module_dir {
            Some(d) => saule_interpreter::module::collect_import_seed(&module, d),
            None => saule_semantic::ModuleSeed::default(),
        };
        let _ = saule_semantic::analyze_with_seed(&module, seed);

        let mut cx = Cx {
            source: &source,
            out: Vec::new(),
            locals: Vec::new(),
            enclosing_class: None,
            user_fns: collect_user_fns(&module),
        };
        cx.visit_module(&module);
        cx.out
            .into_iter()
            .map(|raw| InlayHint {
                position: line_index.position(&source, raw.byte),
                label: InlayHintLabel::String(raw.label),
                kind: Some(raw.kind),
                text_edits: None,
                tooltip: None,
                padding_left: raw.padding_left,
                padding_right: raw.padding_right,
                data: None,
            })
            .collect()
    }
}

struct RawHint {
    byte: usize,
    label: String,
    kind: InlayHintKind,
    padding_left: Option<bool>,
    padding_right: Option<bool>,
}

struct Local {
    name: String,
    ty: Type,
}

struct Cx<'a> {
    #[allow(dead_code)]
    source: &'a str,
    out: Vec<RawHint>,
    locals: Vec<Local>,
    enclosing_class: Option<String>,
    /// Top-level user functions discovered in the module, populated by
    /// `collect_user_fns` before traversal so call sites resolve
    /// regardless of declaration order.
    user_fns: HashMap<String, UserFn>,
}

/// A top-level user function, in the two shapes the hint passes need:
/// its declared parameters (for argument-name hints) and a
/// [`NativeSig`](saule_typeck::sigs::NativeSig) view so a call's return
/// type can be instantiated against the actual argument types.
struct UserFn {
    params: Vec<Param>,
    sig: saule_typeck::sigs::NativeSig,
}

impl<'a> Cx<'a> {
    // ── traversal ───────────────────────────────────────────────────

    fn visit_module(&mut self, module: &Module) {
        for s in &module.stmts {
            self.visit_stmt(s);
        }
    }

    fn visit_stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Local { name, ty, value, name_span, .. } => {
                let resolved_ty = match ty.clone() {
                    // A bare structural annotation (`table` / `function`) is
                    // refined against the initializer so generic natives can
                    // bind the element type (`local nums: table = {1, 2}` ->
                    // `table<integer>`).
                    Some(t) => Some(self.refine_bare_annotation(
                        t,
                        value.as_ref().and_then(|v| self.infer_type(&v.value)),
                    )),
                    // Single binding takes one value; a multi-return (tuple)
                    // collapses to its first component.
                    None => value
                        .as_ref()
                        .and_then(|v| self.infer_type(&v.value))
                        .map(|t| first_value_type(Some(t))),
                };
                if ty.is_none() {
                    if let Some(ref t) = resolved_ty {
                        if let Some(label) = render_type(t) {
                            self.out.push(RawHint {
                                byte: name_span.end,
                                label: format!(": {label}"),
                                kind: InlayHintKind::TYPE,
                                padding_left: None,
                                padding_right: None,
                            });
                        }
                    }
                }
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                self.locals.push(Local {
                    name: name.clone(),
                    ty: resolved_ty.unwrap_or(Type::Named("any".into())),
                });
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.visit_expr(v);
                }
                // Spread the RHS across the bound names (Lua-style: the last
                // value expands its tuple components) so each name resolves
                // to the right type, then emit a positioned hint per name.
                let spread = self.spread_value_types(values);
                for (i, (n, name_span, t)) in names.iter().enumerate() {
                    let resolved = t
                        .clone()
                        .or_else(|| spread.get(i).cloned())
                        .unwrap_or(Type::Named("any".into()));
                    if t.is_none() {
                        if let Some(label) = render_type(&resolved) {
                            self.out.push(RawHint {
                                byte: name_span.end,
                                label: format!(": {label}"),
                                kind: InlayHintKind::TYPE,
                                padding_left: None,
                                padding_right: None,
                            });
                        }
                    }
                    self.locals.push(Local {
                        name: n.clone(),
                        ty: resolved,
                    });
                }
            }
            Stmt::Decl(d) => self.visit_decl(d),
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
                self.visit_block(then_block);
                for (c, b) in elseifs {
                    self.visit_expr(c);
                    self.visit_block(b);
                }
                if let Some(b) = else_block {
                    self.visit_block(b);
                }
            }
            Stmt::While { cond, body } => {
                self.visit_expr(cond);
                self.visit_block(body);
            }
            Stmt::Repeat { body, cond } => {
                self.visit_block(body);
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
                if let Some(s) = step {
                    self.visit_expr(s);
                }
                let mark = self.locals.len();
                self.locals.push(Local {
                    name: var.clone(),
                    ty: var_ty.clone().unwrap_or(Type::Named("integer".into())),
                });
                self.visit_block(body);
                self.locals.truncate(mark);
            }
            Stmt::ForIn { vars, iter, body } => {
                self.visit_expr(iter);
                let mark = self.locals.len();
                for (n, t) in vars {
                    self.locals.push(Local {
                        name: n.clone(),
                        ty: t.clone().unwrap_or(Type::Named("any".into())),
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
                body, catch_body, ..
            } => {
                self.visit_block(body);
                self.visit_block(catch_body);
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn visit_decl(&mut self, d: &Spanned<Decl>) {
        match &d.value {
            Decl::Function { params, body, .. } => {
                self.with_function(params, |this| this.visit_block(body));
            }
            Decl::Class { name, members, .. } => {
                let prev = self.enclosing_class.replace(name.clone());
                for m in members {
                    if let ClassMember::Method(meth) = &m.value {
                        self.visit_method(meth);
                    }
                    if let ClassMember::Field { default: Some(d), .. } = &m.value {
                        self.visit_expr(d);
                    }
                }
                self.enclosing_class = prev;
            }
            Decl::Enum { methods, .. } => {
                for m in methods {
                    self.visit_method(m);
                }
            }
            Decl::Interface { .. } | Decl::Import { .. } => {}
        }
    }

    fn visit_method(&mut self, m: &Method) {
        self.with_function(&m.params, |this| this.visit_block(&m.body));
    }

    fn with_function(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let saved = std::mem::take(&mut self.locals);
        for p in params {
            self.locals.push(Local {
                name: p.name.clone(),
                ty: p.ty.clone(),
            });
        }
        body(self);
        self.locals = saved;
    }

    fn visit_block(&mut self, body: &[Spanned<Stmt>]) {
        let mark = self.locals.len();
        for s in body {
            self.visit_stmt(s);
        }
        self.locals.truncate(mark);
    }

    fn visit_expr(&mut self, e: &Spanned<Expr>) {
        match &e.value {
            Expr::Cast { value, .. } => self.visit_expr(value),
            Expr::Call { callee, args } => {
                self.visit_expr(callee);
                let params = self.callee_params(&callee.value);
                self.emit_param_hints(args, params.as_ref());
                for a in args {
                    self.visit_call_arg(a);
                }
            }
            Expr::MethodCall { obj, method, args } => {
                self.visit_expr(obj);
                let params = self.method_callee_params(&obj.value, method);
                self.emit_param_hints(args, params.as_ref());
                for a in args {
                    self.visit_call_arg(a);
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
                let saved = std::mem::take(&mut self.locals);
                for p in params {
                    self.locals.push(Local {
                        name: p.name.clone(),
                        ty: p.ty.clone(),
                    });
                }
                match body {
                    LambdaBody::Expr(b) => self.visit_expr(b),
                    LambdaBody::Block(b) => self.visit_block(b),
                }
                self.locals = saved;
            }
            Expr::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    match &arm.body {
                        MatchBody::Expr(e) => self.visit_expr(e),
                        MatchBody::Block(b) => self.visit_block(b),
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

    fn visit_call_arg(&mut self, a: &CallArg) {
        match a {
            CallArg::Positional(e) | CallArg::Named { value: e, .. } => self.visit_expr(e),
        }
    }

    // ── inlay emission ──────────────────────────────────────────────

    /// Emit `name:` hints in front of every positional argument whose
    /// matching parameter we could resolve. Suppressed when the arg
    /// is itself the bare parameter name (`add(a, b)` would be noisy)
    /// or already named at the source level.
    fn emit_param_hints(&mut self, args: &[CallArg], params: Option<&CalleeParams>) {
        let Some(params) = params else { return };
        // Project the two shapes down to a `(name, is_variadic)` slice
        // so the rest of the loop can treat them uniformly. Native
        // sigs don't carry per-slot variadic info — at most one
        // trailing variadic slot — so we mark only that last one.
        let slots: Vec<(String, bool)> = match params {
            CalleeParams::Named(ps) => ps
                .iter()
                .map(|p| (p.name.clone(), p.variadic))
                .collect(),
            CalleeParams::Native { names, has_variadic } => names
                .iter()
                .enumerate()
                .map(|(i, n)| (n.clone(), *has_variadic && i + 1 == names.len()))
                .collect(),
        };
        let mut pi = 0;
        for arg in args {
            match arg {
                CallArg::Named { .. } => {
                    pi += 1;
                }
                CallArg::Positional(value) => {
                    let Some((name, is_var)) = slots.get(pi) else { break };
                    pi += 1;
                    if *is_var {
                        break;
                    }
                    if let Expr::Ident(n) = &value.value {
                        if n == name {
                            continue;
                        }
                    }
                    if name.is_empty() {
                        continue;
                    }
                    self.out.push(RawHint {
                        byte: value.span.start,
                        label: format!("{name}:"),
                        kind: InlayHintKind::PARAMETER,
                        padding_left: None,
                        padding_right: Some(true),
                    });
                }
            }
        }
    }

    // ── lightweight type inference (locals + constructor calls) ─────

    /// Expand a value-expression list into the flat list of value types it
    /// yields, using Lua-style multi-assign semantics (mirrors the
    /// interpreter): every expression contributes one value except the last,
    /// whose tuple components (a multi-return) spread into several.
    /// Non-final expressions sit in single-value context, so a tuple there
    /// collapses to its first component.
    fn spread_value_types(&self, values: &[Spanned<Expr>]) -> Vec<Type> {
        let mut out = Vec::new();
        let n = values.len();
        for (i, v) in values.iter().enumerate() {
            let ty = self.infer_type(&v.value);
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

    fn infer_type(&self, e: &Expr) -> Option<Type> {
        match e {
            Expr::Int(_) => Some(Type::Named("integer".into())),
            Expr::Float(_) => Some(Type::Named("float".into())),
            Expr::Bool(_) => Some(Type::Named("boolean".into())),
            Expr::Str(_) => Some(Type::Named("string".into())),
            Expr::Nil => None,
            Expr::Ident(name) => self
                .locals
                .iter()
                .rev()
                .find(|l| l.name == *name)
                .map(|l| l.ty.clone()),
            Expr::Call { callee, args } => {
                if let Expr::Ident(name) = &callee.value {
                    if with_classes(|r| r.contains_key(name)) {
                        return Some(Type::Named(name.clone()));
                    }
                    // Sibling top-level `fn` — bind its type parameters
                    // from the actual arguments, exactly as for a generic
                    // native.
                    if let Some(f) = self.user_fns.get(name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&f.sig, &arg_types)
                            .into_iter()
                            .next()
                            // A type parameter the arguments didn't pin
                            // down comes back as its own bare name — no
                            // hint beats a hint reading `: table<U>`.
                            .filter(|t| {
                                !saule_typeck::sigs::mentions_unbound_param(t, &f.sig.type_params)
                            });
                    }
                    if let Some(sig) = saule_typeck::sigs::lookup(name) {
                        let arg_types = self.positional_arg_types(args);
                        return saule_typeck::sigs::instantiate_returns(&sig, &arg_types)
                            .into_iter()
                            .next();
                    }
                }
                // `recv.method(args)` — dot-call on a module or instance.
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
            Expr::Self_ => self.enclosing_class.clone().map(Type::Named),
            Expr::Table(entries) => Some(self.infer_table_literal(entries)),
            // `#xs` / `not x` have a type regardless of their operand;
            // `-x` takes the operand's. Cheap, and it's what lets a
            // callback body like `s => #s` pin down a generic result.
            Expr::Unary { op, rhs } => match op {
                saule_ast::UnaryOp::Len => Some(Type::Named("integer".into())),
                saule_ast::UnaryOp::Not => Some(Type::Named("boolean".into())),
                saule_ast::UnaryOp::Neg => self.infer_type(&rhs.value),
            },
            Expr::Lambda { params, return_ty, body } => Some(Type::Function {
                params: params.iter().map(|p| p.ty.clone()).collect(),
                ret: Box::new(
                    return_ty
                        .clone()
                        .or_else(|| self.infer_lambda_return(params, body))
                        .unwrap_or_else(|| Type::Named("any".into())),
                ),
            }),
            _ => None,
        }
    }

    /// Best-effort return type of an unannotated expression-bodied
    /// lambda, so a generic call like `map(items, s => #s)` can bind the
    /// callback's result type. Mirrors the hover walker's helper of the
    /// same name, including its two limits: block bodies are skipped, and
    /// so is any lambda whose parameters shadow an in-scope binding (the
    /// body is inferred against the enclosing scope, where a shadowed
    /// name would resolve to the wrong local).
    fn infer_lambda_return(&self, params: &[Param], body: &LambdaBody) -> Option<Type> {
        let LambdaBody::Expr(e) = body else {
            return None;
        };
        if params
            .iter()
            .any(|p| self.locals.iter().any(|l| l.name == p.name))
        {
            return None;
        }
        Some(first_value_type(self.infer_type(&e.value)))
    }

    /// Infer a `table<V>` (array literal) or `table<K, V>` (map literal)
    /// from a table constructor's entries so generic natives like
    /// `Util.map(table<T>, …)` bind their element type. Falls back to a bare
    /// `table` when entries are empty or their types disagree.
    fn infer_table_literal(&self, entries: &[TableEntry]) -> Type {
        let mut value_ty: Option<Type> = None;
        let mut key_ty: Option<Type> = None;
        let mut has_field = false;
        let mut consistent = true;
        for entry in entries {
            let (k, v) = match entry {
                TableEntry::Positional(v) => (None, self.infer_type(&v.value)),
                TableEntry::Field { key, value } => {
                    has_field = true;
                    (self.infer_type(&key.value), self.infer_type(&value.value))
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
                key: if has_field { key_ty.map(Box::new) } else { None },
                value: Box::new(v),
            },
            _ => Type::Named("table".into()),
        }
    }

    /// Refine a bare structural annotation (`table` / `function`) against the
    /// initializer's inferred shape: `local nums: table = {1, 2}` becomes
    /// `table<integer>`. Leaves the declared type untouched on a mismatch.
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

    /// Infer the types of a call's positional arguments (in order;
    /// `None` where inference can't produce a type). Named arguments are
    /// skipped — mirrors how the typechecker binds generics from
    /// positional args only.
    fn positional_arg_types(&self, args: &[CallArg]) -> Vec<Option<Type>> {
        args.iter()
            .filter_map(|a| match a {
                CallArg::Positional(e) => Some(self.infer_type(&e.value)),
                CallArg::Named { .. } => None,
            })
            .collect()
    }

    /// Best-effort: figure out which class a member-access receiver
    /// refers to. Mirrors the hover walker's `receiver_class` but
    /// limited to the cases inlay hints actually need.
    fn receiver_class(&self, obj: &Expr) -> Option<String> {
        match obj {
            Expr::Self_ => self.enclosing_class.clone(),
            Expr::Ident(name) => {
                if let Some(local) = self
                    .locals
                    .iter()
                    .rev()
                    .find(|l| l.name == *name)
                {
                    if let Type::Named(n) = &local.ty {
                        return Some(n.clone());
                    }
                }
                if with_classes(|r| r.contains_key(name)) {
                    return Some(name.clone());
                }
                None
            }
            Expr::Call { callee, .. } => {
                if let Expr::Ident(n) = &callee.value {
                    if with_classes(|r| r.contains_key(n)) {
                        return Some(n.clone());
                    }
                }
                None
            }
            Expr::MethodCall { obj, method, .. } => {
                let cls = self.receiver_class(&obj.value)?;
                let sig = lookup_method(&cls, method)?;
                if let Type::Named(n) = sig.return_ty? {
                    Some(n)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn callee_params(&self, callee: &Expr) -> Option<CalleeParams> {
        match callee {
            Expr::Ident(name) => {
                if with_classes(|r| r.contains_key(name)) {
                    return lookup_method(name, "init")
                        .map(|sig| CalleeParams::Named(sig.params));
                }
                if let Some(class) = &self.enclosing_class {
                    if let Some(sig) = lookup_method(class, name) {
                        return Some(CalleeParams::Named(sig.params));
                    }
                }
                if let Some(f) = self.user_fns.get(name) {
                    return Some(CalleeParams::Named(f.params.clone()));
                }
                // Bare native (`assert`, `tonumber`, ...): synthesise
                // names from the registered native sig so the user
                // gets `assert(cond: true, message: ...)`.
                if let Some(native) = saule_typeck::sigs::lookup(name) {
                    return Some(CalleeParams::Native {
                        names: super::native_names::param_names(name, &native),
                        has_variadic: native.variadic.is_some(),
                    });
                }
                None
            }
            Expr::Member { obj, name } => {
                if let Some(class) = self.receiver_class(&obj.value) {
                    if let Some(sig) = lookup_method(&class, name) {
                        return Some(CalleeParams::Named(sig.params));
                    }
                    let qname = format!("{class}.{name}");
                    if let Some(native) = saule_typeck::sigs::lookup(&qname) {
                        return Some(CalleeParams::Native {
                            names: super::native_names::param_names(&qname, &native),
                            has_variadic: native.variadic.is_some(),
                        });
                    }
                }
                // Stdlib module call: `Math.floor(2)` — receiver is a
                // bare identifier registered as a module.
                if let Expr::Ident(recv) = &obj.value {
                    let qname = format!("{recv}.{name}");
                    if let Some(native) = saule_typeck::sigs::lookup(&qname) {
                        return Some(CalleeParams::Native {
                            names: super::native_names::param_names(&qname, &native),
                            has_variadic: native.variadic.is_some(),
                        });
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Resolve the parameter list for `obj.method(...)`. Mirrors the
    /// Member-arm in [`Self::callee_params`] but specialised for
    /// `Expr::MethodCall` whose AST keeps `method` as a bare string.
    fn method_callee_params(&self, obj: &Expr, method: &str) -> Option<CalleeParams> {
        if let Some(class) = self.receiver_class(obj) {
            if let Some(sig) = lookup_method(&class, method) {
                return Some(CalleeParams::Named(sig.params));
            }
            let qname = format!("{class}.{method}");
            if let Some(native) = saule_typeck::sigs::lookup(&qname) {
                return Some(CalleeParams::Native {
                    names: super::native_names::param_names(&qname, &native),
                    has_variadic: native.variadic.is_some(),
                });
            }
        }
        if let Expr::Ident(recv) = obj {
            let qname = format!("{recv}.{method}");
            if let Some(native) = saule_typeck::sigs::lookup(&qname) {
                return Some(CalleeParams::Native {
                    names: super::native_names::param_names(&qname, &native),
                    has_variadic: native.variadic.is_some(),
                });
            }
        }
        None
    }
}

/// Either a list of AST `Param`s (which may have explicit names and
/// types) or a list of synthesised names from a native sig together
/// with a flag marking whether the trailing slot is variadic. The
/// two shapes are kept apart so the caller can decide whether to
/// consult `param.name` and `param.variadic` (only meaningful for
/// named params).
enum CalleeParams {
    Named(Vec<Param>),
    Native { names: Vec<String>, has_variadic: bool },
}

/// Pre-pass: collect every top-level `fn name(...)` declaration so the
/// param-hint walker can resolve free-call expressions whose target
/// is a user-defined function (not a class init, not a stdlib native).
fn collect_user_fns(module: &Module) -> HashMap<String, UserFn> {
    let mut out = HashMap::new();
    for s in &module.stmts {
        if let Stmt::Decl(d) = &s.value {
            if let Decl::Function {
                name, type_params, params, return_ty, ..
            } = &d.value
            {
                out.insert(
                    name.clone(),
                    UserFn {
                        params: params.clone(),
                        sig: saule_typeck::sigs::NativeSig {
                            type_params: type_params.clone(),
                            params: params.iter().map(|p| p.ty.clone()).collect(),
                            variadic: None,
                            returns: return_ty.clone().into_iter().collect(),
                        },
                    },
                );
            }
        }
    }
    out
}

/// Render a `Type` as a short label suitable for an inlay hint. Returns
/// `None` for types we don't want to surface (`any` is noise; unnamed
/// fallbacks would be misleading).
fn render_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) if n == "any" || n.is_empty() => None,
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => render_type(inner).map(|s| format!("{s}?")),
        // Element types are worth showing — `local xs = map(names, …)`
        // reads much better as `xs : table<integer>` than as nothing at
        // all. Rendered only when every component renders, so an `any`
        // anywhere inside still suppresses the whole hint rather than
        // spending screen width on `table<any>`.
        Type::Table { key: None, value } => render_type(value).map(|v| format!("table<{v}>")),
        Type::Table { key: Some(key), value } => match (render_type(key), render_type(value)) {
            (Some(k), Some(v)) => Some(format!("table<{k}, {v}>")),
            _ => None,
        },
        _ => None,
    }
}

/// Collapse an inferred type to the single value it yields in a single-value
/// context: a multi-return tuple becomes its first component, anything else
/// passes through, and `None` becomes `any`.
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

#[cfg(test)]
mod tests {
    //! Inlay hint tests. We bypass `Backend` entirely and run the
    //! walker against a parsed+analysed module, then assert on the
    //! resulting `(kind, byte, label)` triples.

    use super::*;

    fn raw_hints(src: &str) -> Vec<(InlayHintKind, usize, String)> {
        let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
        let module = saule_parser::parse(tokens).expect("parse");
        let _ = saule_semantic::analyze(&module);
        let mut cx = Cx {
            source: src,
            out: Vec::new(),
            locals: Vec::new(),
            enclosing_class: None,
            user_fns: collect_user_fns(&module),
        };
        cx.visit_module(&module);
        cx.out
            .into_iter()
            .map(|h| (h.kind, h.byte, h.label))
            .collect()
    }

    /// A call to a top-level user function carries its declared return
    /// type into the local's hint.
    #[test]
    fn type_hint_from_user_function_return() {
        let src = "fn first(items: table<string>) -> string\n  return items[1]\nend\n\nlocal x = first({\"a\"})\n";
        let hints = raw_hints(src);
        assert!(
            hints
                .iter()
                .any(|(k, _, l)| *k == InlayHintKind::TYPE && l == ": string"),
            "{hints:?}"
        );
    }

    /// A generic user function binds its type parameters from the actual
    /// arguments — including the result type of an expression-bodied
    /// callback.
    #[test]
    fn type_hint_from_generic_user_function() {
        let src = "fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n  local out: table<U> = {}\n  return out\nend\n\nlocal lengths = map({\"a\"}, s => #s)\n";
        let hints = raw_hints(src);
        assert!(
            hints
                .iter()
                .any(|(k, _, l)| *k == InlayHintKind::TYPE && l == ": table<integer>"),
            "{hints:?}"
        );
    }

    /// When the arguments never pin a type parameter down, no hint at all
    /// — a label reading `: table<U>` names something the user can't act
    /// on.
    #[test]
    fn no_type_hint_when_type_param_stays_unbound() {
        let src = "fn make<T>(n: integer) -> table<T>\n  local out: table<T> = {}\n  return out\nend\n\nlocal xs = make(3)\n";
        let hints = raw_hints(src);
        assert!(
            !hints.iter().any(|(k, _, _)| *k == InlayHintKind::TYPE),
            "{hints:?}"
        );
    }

    #[test]
    fn type_hint_for_inferred_local_from_constructor() {
        let src = "class Point\n  x: integer = 0\nend\n\nfn main()\n  local p = Point()\nend\n";
        let hints = raw_hints(src);
        let type_hints: Vec<_> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::TYPE)
            .collect();
        assert_eq!(type_hints.len(), 1, "got {hints:?}");
        assert_eq!(type_hints[0].2, ": Point");
    }

    #[test]
    fn no_type_hint_when_already_annotated() {
        let src = "fn main()\n  local x: integer = 1\nend\n";
        let hints = raw_hints(src);
        let type_hints: Vec<_> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::TYPE)
            .collect();
        assert!(type_hints.is_empty(), "got {hints:?}");
    }

    #[test]
    fn type_hint_for_int_literal() {
        let src = "fn main()\n  local n = 42\nend\n";
        let hints = raw_hints(src);
        let labels: Vec<&String> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::TYPE)
            .map(|(_, _, l)| l)
            .collect();
        assert_eq!(labels, vec![&": integer".to_string()]);
    }

    #[test]
    fn parameter_hint_for_positional_call_within_class() {
        // Free top-level functions aren't resolved by inlay yet, so
        // exercise the param-hint path through a sibling-class call.
        let src = "class Calc\n  fn add(a: integer, b: integer) -> integer\n    return a + b\n  end\n  fn main()\n    local r = self.add(1, 2)\n  end\nend\n";
        let hints = raw_hints(src);
        let labels: Vec<&String> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
            .map(|(_, _, l)| l)
            .collect();
        assert!(labels.contains(&&"a:".to_string()), "got {hints:?}");
        assert!(labels.contains(&&"b:".to_string()), "got {hints:?}");
    }

    #[test]
    fn parameter_hint_suppressed_when_arg_matches_param_name() {
        let src = "class Calc\n  fn add(a: integer, b: integer) -> integer\n    return a + b\n  end\n  fn main()\n    local a = 1\n    local b = 2\n    local r = self.add(a, b)\n  end\nend\n";
        let hints = raw_hints(src);
        let param_hints: Vec<_> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
            .collect();
        assert!(param_hints.is_empty(), "got {param_hints:?}");
    }

    #[test]
    fn parameter_hint_for_class_constructor() {
        let src = "class Point\n  x: integer = 0\n  y: integer = 0\n  fn init(x: integer, y: integer)\n    self.x = x\n    self.y = y\n  end\nend\n\nfn main()\n  local p = Point(1, 2)\nend\n";
        let hints = raw_hints(src);
        let labels: Vec<&String> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
            .map(|(_, _, l)| l)
            .collect();
        assert!(labels.contains(&&"x:".to_string()), "got {hints:?}");
        assert!(labels.contains(&&"y:".to_string()), "got {hints:?}");
    }

    fn init_stdlib() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            saule_interpreter::init();
        });
    }

    #[test]
    fn parameter_hint_for_free_top_level_fn() {
        let src = "fn add(x: integer, y: integer) -> integer\n  return x + y\nend\n\nfn main()\n  local r = add(1, 2)\nend\n";
        let hints = raw_hints(src);
        let labels: Vec<&String> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
            .map(|(_, _, l)| l)
            .collect();
        assert!(labels.contains(&&"x:".to_string()), "got {hints:?}");
        assert!(labels.contains(&&"y:".to_string()), "got {hints:?}");
    }

    #[test]
    fn parameter_hint_for_stdlib_module_call() {
        init_stdlib();
        // `String.find(s, pattern, init?)` — first two positionals get
        // `s:` and `pattern:` from the static names table.
        let src = "fn main()\n  local i = String.find(\"hello\", \"l\")\nend\n";
        let hints = raw_hints(src);
        let labels: Vec<&String> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
            .map(|(_, _, l)| l)
            .collect();
        assert!(labels.contains(&&"s:".to_string()), "got {hints:?}");
        assert!(labels.contains(&&"pattern:".to_string()), "got {hints:?}");
    }

    #[test]
    fn parameter_hint_suppressed_for_println() {
        init_stdlib();
        // `println` is registered as a purely-variadic native
        // (`println(...any)`). The walker treats variadic slots as
        // unlabel-able, so a `println("hello")` should produce no
        // parameter inlay hint — labelling the first arg `value:`
        // when there are no fixed positional slots would be noise.
        let src = "fn main()\n  println(\"hello\")\nend\n";
        let hints = raw_hints(src);
        let labels: Vec<&String> = hints
            .iter()
            .filter(|(k, _, _)| *k == InlayHintKind::PARAMETER)
            .map(|(_, _, l)| l)
            .collect();
        assert!(labels.is_empty(), "expected no param hints, got {hints:?}");
    }
}

