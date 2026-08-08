//! Walking expressions, call arguments and patterns, and rendering
//! the markdown shown for the expression under the cursor.

use crate::hover::render::{
    render_method_sig, render_param, render_type, render_variant_pattern, with_param_doc,
};
use crate::hover::util::{
    contains, locate_word_in, render_unknown_member, resolve_member, strip_nullable_type,
};
use saule_ast::{Expr, Pattern, Spanned, Type};

use super::*;

impl<'a> Cx<'a> {
    pub(crate) fn visit_expr(&mut self, e: &Spanned<Expr>) {
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
                let sig = self.callee_sig(&callee.value);
                // What each slot expects, with the callee's generics
                // bound from the arguments — `map(names, s => #s)` gives
                // `s` the element type of `names`, not `any`.
                let expected = sig
                    .as_ref()
                    .map(|s| s.expected_arg_types(&self.positional_arg_types(args)))
                    .unwrap_or_default();
                // Slot assignment follows the same rule the typechecker and
                // interpreter use, so a trailing block after a named argument
                // is read against the parameter it actually binds to.
                let param_slots = sig
                    .as_ref()
                    .map(|s| saule_ast::param_slots(&s.params))
                    .unwrap_or_default();
                let slots = saule_ast::resolve_arg_slots(args, &param_slots);
                for (a, slot) in args.iter().zip(slots.iter()) {
                    let want = slot.and_then(|i| expected.get(i).cloned().flatten());
                    self.visit_call_arg_with_params(a, sig.as_ref(), want.as_ref());
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
            Expr::Lambda {
                params,
                return_ty,
                body,
            } => self.visit_lambda(e, params, return_ty, body),
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
                // Each stage's generics are bound from the value that
                // reaches it, so `:filter(x => …)` over a `table<integer>`
                // reads `x` as an `integer`.
                let expectations =
                    crate::exprty::pipe_stage_expectations(self, &source.value, stages);
                for (si, st) in stages.iter().enumerate() {
                    // A stage is a call to the free function it names,
                    // so the name hovers as that function. Without this
                    // the cursor fell through to the whole pipeline and
                    // was answered with the type the chain produces.
                    if let Some(span) = locate_word_in(self.source, &st.span, &st.name)
                        && contains(&span, self.offset)
                        && let Some(md) = self.resolve_ident(&st.name)
                    {
                        self.record(span, md);
                    }
                    // Stdlib pipe stages (`|> Math.sqrt`) carry only
                    // positional types — no parameter names — so we
                    // can't resolve named-arg keys against them.
                    for (ai, a) in st.args.iter().enumerate() {
                        let want = expectations
                            .get(si)
                            .and_then(|v| v.get(ai))
                            .cloned()
                            .flatten();
                        self.visit_call_arg_with_params(a, None, want.as_ref());
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
    pub(crate) fn visit_call_arg(&mut self, a: &saule_ast::CallArg) {
        match a {
            saule_ast::CallArg::Positional(e) => self.visit_expr(e),
            saule_ast::CallArg::Named { value, .. } => self.visit_expr(value),
        }
    }

    /// Walk a lambda with the parameter list it should be *read* with.
    ///
    /// Split out from the `Expr::Lambda` arm so a call site can hand in
    /// parameters refined from the slot's expected type: an omitted
    /// annotation parses as `any`, and the callee's signature is the only
    /// place the real type comes from.
    pub(crate) fn visit_lambda(
        &mut self,
        e: &Spanned<Expr>,
        params: &[Param],
        return_ty: &Option<Type>,
        body: &saule_ast::LambdaBody,
    ) {
        for p in params {
            self.record(p.span.clone(), render_param(p));
            // `fn(s: Storage)` — the ascription's own head is a hover
            // target, exactly as on a `fn` declaration.
            self.record_type_idents_in(&p.ty, &p.span);
            if let Some(def) = &p.default {
                self.visit_expr(def);
            }
        }
        if let Some(rt) = return_ty {
            // The return type sits between the parameter list and the body.
            let after = params.last().map(|p| p.span.end).unwrap_or(e.span.start);
            let before = match body {
                saule_ast::LambdaBody::Expr(b) => b.span.start,
                saule_ast::LambdaBody::Block(b) => {
                    b.first().map(|s| s.span.start).unwrap_or(e.span.end)
                }
            };
            self.record_type_idents_in(rt, &(after..before.max(after)));
        }
        let params = params.to_vec();
        let return_ty = return_ty.clone();
        self.enter_lambda(&params, return_ty.as_ref(), |this| match body {
            saule_ast::LambdaBody::Expr(b) => this.visit_expr(b),
            saule_ast::LambdaBody::Block(b) => this.visit_block(b),
        });
    }

    /// [`visit_expr`] carrying the type this position expects, so a
    /// lambda's untyped parameters are read as the callee declared them.
    pub(crate) fn visit_expr_expecting(&mut self, e: &Spanned<Expr>, expected: Option<&Type>) {
        if let Expr::Lambda {
            params,
            return_ty,
            body,
        } = &e.value
            && expected.is_some()
        {
            if !contains(&e.span, self.offset) {
                return;
            }
            if let Some(md) = self.expr_md(&e.value) {
                self.record(e.span.clone(), md);
            }
            let refined = crate::exprty::refine_lambda_params(params, expected);
            self.visit_lambda(e, &refined, return_ty, body);
            return;
        }
        self.visit_expr(e);
    }

    /// Like [`visit_call_arg`] but also surfaces hover info on a named
    /// argument's *key* (`storage.add(item, dueDate: due)` -> hovering
    /// on `dueDate` shows the parameter declaration). `callee` carries
    /// the resolved callee when we found one, so the key can be rendered
    /// as the parameter it actually is.
    pub(crate) fn visit_call_arg_with_params(
        &mut self,
        a: &saule_ast::CallArg,
        callee: Option<&CalleeSig>,
        expected: Option<&Type>,
    ) {
        match a {
            saule_ast::CallArg::Positional(e) => self.visit_expr_expecting(e, expected),
            saule_ast::CallArg::Named { name, value } => {
                let key_search = value.span.start.saturating_sub(name.len() + 4)..value.span.start;
                if let Some(span) = locate_word_in(self.source, &key_search, name)
                    && contains(&span, self.offset)
                {
                    self.record(span, self.render_named_arg(name, callee));
                }
                self.visit_expr_expecting(value, expected);
            }
        }
    }

    /// The hover for a named argument's key.
    ///
    /// A named-argument key *is* the callee's parameter, so it renders
    /// as one — same `(parameter)` label, same rendering of a default
    /// as the declaration site, and qualified by the callee's name.
    ///
    /// The qualifier is what makes this readable in Flutter-shaped code.
    /// A single build method here holds eight `child:` keys belonging to
    /// six different widgets; unqualified they all rendered the identical
    /// `child: Widget?` blurb, so the popup confirmed nothing about
    /// which one the cursor was on. `Container.child` does.
    pub(crate) fn render_named_arg(&self, name: &str, callee: Option<&CalleeSig>) -> String {
        let Some(c) = callee else {
            // Callee unresolved — say only what we actually know rather
            // than inventing a type.
            return format!("```saule\n(parameter) {name}\n```");
        };
        let Some(p) = c.params.iter().find(|p| p.name == name) else {
            // Callee resolved but has no such parameter. Naming the miss
            // is the same service `render_unknown_member` performs, and
            // it stops a wider node from answering in its place.
            return format!(
                "```saule\n(unknown) {owner}.{name}\n```\nNo parameter `{name}` on `{owner}`.",
                owner = c.display
            );
        };
        let mut s = String::from("```saule\n(parameter) ");
        if !c.display.is_empty() {
            s.push_str(&c.display);
            s.push('.');
        }
        if p.variadic {
            s.push_str("...");
        }
        s.push_str(&p.name);
        s.push_str(": ");
        s.push_str(&render_type(&p.ty));
        s.push_str(&crate::hover::render::render_default_suffix(p));
        s.push_str("\n```");
        with_param_doc(s, c.doc.as_ref(), name)
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
    pub(crate) fn visit_pattern(&mut self, p: &Spanned<Pattern>) {
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
    pub(crate) fn bind_pattern(&mut self, pat: &Pattern, scrut_ty: Option<&Type>) {
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
    /// and unknown names.
    ///
    /// `None` does **not** mean "no hover here". [`Cx::record`] keeps the
    /// narrowest span containing the cursor, so declining to answer for a
    /// node just lets a wider one — in practice the enclosing `fn` — win
    /// instead. For a token the user pointed at directly that reads as a
    /// confident, wrong answer about an unrelated symbol.
    ///
    /// So: return `None` only where the node itself is genuinely nothing
    /// to talk about (a literal, a `nil`). Where we know enough to say
    /// something is *wrong* — a member that isn't on its receiver — say
    /// that, via [`render_unknown_member`]. It is both the more useful
    /// answer and the one that stops the fallback.
    pub(crate) fn expr_md(&self, expr: &Expr) -> Option<String> {
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
                Some(
                    resolve_member(&class, name, false, &self.imports.docs)
                        .unwrap_or_else(|| render_unknown_member(&class, name)),
                )
            }
            // A literal is its own documentation. Answering `(expr):
            // string` for a cursor inside a sentence of prose is the
            // kind of hover that fires where the reader asked nothing,
            // and the type it reports was never in doubt.
            Expr::Int(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Str(_) | Expr::Nil => None,
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
}
