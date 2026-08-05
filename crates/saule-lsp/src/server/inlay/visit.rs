//! The traversal: statements, declarations, methods, blocks and
//! expressions, plus the parameter-name hint emission at call sites.

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, LambdaBody, MatchBody, Method, Module, Param, Spanned, Stmt,
    TableEntry, Type,
};
use tower_lsp::lsp_types::InlayHintKind;

use super::*;

impl<'a> Cx<'a> {
    pub(crate) fn visit_module(&mut self, module: &Module) {
        for s in &module.stmts {
            self.visit_stmt(s);
        }
    }

    /// Walk a lambda body, optionally against the type its slot expects.
    ///
    /// Two things happen here. The enclosing scope is *kept* — a lambda
    /// is a closure, so a hint inside one has to resolve the names around
    /// it; starting fresh meant a local initialised from a captured
    /// variable got no hint at all. And an omitted parameter type is
    /// filled in from `expected`, which is the only place it can come
    /// from, so hints inside the body see the real type instead of `any`.
    pub(crate) fn visit_lambda(
        &mut self,
        params: &[Param],
        body: &LambdaBody,
        expected: Option<&Type>,
    ) {
        let mark = self.locals.len();
        for p in crate::exprty::refine_lambda_params(params, expected) {
            self.locals.push(Local {
                name: p.name.clone(),
                ty: p.ty.clone(),
            });
        }
        match body {
            LambdaBody::Expr(b) => self.visit_expr(b),
            LambdaBody::Block(b) => self.visit_block(b),
        }
        self.locals.truncate(mark);
    }

    /// [`visit_expr`] carrying the type this position expects, so a
    /// lambda's untyped parameters are walked as the callee declared them.
    pub(crate) fn visit_expr_expecting(&mut self, e: &Spanned<Expr>, expected: Option<&Type>) {
        if let Expr::Lambda { params, body, .. } = &e.value
            && expected.is_some()
        {
            self.visit_lambda(params, body, expected);
            return;
        }
        self.visit_expr(e);
    }

    pub(crate) fn visit_stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Local {
                name,
                ty,
                value,
                name_span,
                ..
            } => {
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
                if ty.is_none()
                    && let Some(ref t) = resolved_ty
                    && let Some(label) = render_type(t)
                {
                    self.out.push(RawHint {
                        byte: name_span.end,
                        label: format!(": {label}"),
                        kind: InlayHintKind::TYPE,
                        padding_left: None,
                        padding_right: None,
                    });
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
                    if t.is_none()
                        && let Some(label) = render_type(&resolved)
                    {
                        self.out.push(RawHint {
                            byte: name_span.end,
                            label: format!(": {label}"),
                            kind: InlayHintKind::TYPE,
                            padding_left: None,
                            padding_right: None,
                        });
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

    pub(crate) fn visit_decl(&mut self, d: &Spanned<Decl>) {
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
                    if let ClassMember::Field {
                        default: Some(d), ..
                    } = &m.value
                    {
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
            Decl::Variable { value, .. } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
            }
            Decl::Interface { .. } | Decl::Import { .. } => {}
        }
    }

    pub(crate) fn visit_method(&mut self, m: &Method) {
        self.with_function(&m.params, |this| this.visit_block(&m.body));
    }

    pub(crate) fn with_function(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
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

    pub(crate) fn visit_block(&mut self, body: &[Spanned<Stmt>]) {
        let mark = self.locals.len();
        for s in body {
            self.visit_stmt(s);
        }
        self.locals.truncate(mark);
    }

    pub(crate) fn visit_expr(&mut self, e: &Spanned<Expr>) {
        match &e.value {
            Expr::Cast { value, .. } => self.visit_expr(value),
            Expr::Call { callee, args } => {
                self.visit_expr(callee);
                let params = self.callee_params(&callee.value);
                self.emit_param_hints(args, params.as_ref());
                let expected = self.expected_arg_types(&callee.value, args);
                for (i, a) in args.iter().enumerate() {
                    self.visit_call_arg_expecting(a, expected.get(i).and_then(|t| t.as_ref()));
                }
            }
            Expr::Pipe { source, stages } => {
                self.visit_expr(source);
                // Each stage's generics bind from the value reaching it,
                // so an untyped lambda argument gets a real parameter type.
                let expectations =
                    crate::exprty::pipe_stage_expectations(self, &source.value, stages);
                for (si, st) in stages.iter().enumerate() {
                    for (ai, a) in st.args.iter().enumerate() {
                        let want = expectations
                            .get(si)
                            .and_then(|v| v.get(ai))
                            .cloned()
                            .flatten();
                        self.visit_call_arg_expecting(a, want.as_ref());
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
            Expr::Lambda { params, body, .. } => self.visit_lambda(params, body, None),
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

    pub(crate) fn visit_call_arg_expecting(&mut self, a: &CallArg, expected: Option<&Type>) {
        match a {
            CallArg::Positional(e) | CallArg::Named { value: e, .. } => {
                self.visit_expr_expecting(e, expected)
            }
        }
    }

    // ── inlay emission ──────────────────────────────────────────────

    /// Emit `name:` hints in front of every positional argument whose
    /// matching parameter we could resolve. Suppressed when the arg
    /// is itself the bare parameter name (`add(a, b)` would be noisy)
    /// or already named at the source level.
    pub(crate) fn emit_param_hints(&mut self, args: &[CallArg], params: Option<&CalleeParams>) {
        let Some(params) = params else { return };
        // Project the two shapes down to a `(name, is_variadic)` slice
        // so the rest of the loop can treat them uniformly. Native
        // sigs don't carry per-slot variadic info — at most one
        // trailing variadic slot — so we mark only that last one.
        let slots: Vec<(String, bool)> = match params {
            CalleeParams::Named(ps) => ps.iter().map(|p| (p.name.clone(), p.variadic)).collect(),
            CalleeParams::Native {
                names,
                has_variadic,
            } => names
                .iter()
                .enumerate()
                .map(|(i, n)| (n.clone(), *has_variadic && i + 1 == names.len()))
                .collect(),
        };
        let mut pi = 0;
        let last = args.len().saturating_sub(1);
        for (i, arg) in args.iter().enumerate() {
            // A trailing block's hint would render on the `do` keyword, past
            // the closing paren — and which parameter it fills is already
            // plain from the syntax. Skip it rather than label it. Only the
            // *final* argument can be one: a block-bodied lambda earlier in
            // the list is an ordinary argument and still gets its hint.
            if i == last && arg.is_trailing_block() {
                continue;
            }
            match arg {
                CallArg::Named { .. } => {
                    pi += 1;
                }
                CallArg::Positional(value) => {
                    let Some((name, is_var)) = slots.get(pi) else {
                        break;
                    };
                    pi += 1;
                    if *is_var {
                        break;
                    }
                    if let Expr::Ident(n) = &value.value
                        && n == name
                    {
                        continue;
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
}
