//! Local scope tracking and the lightweight type inference the walk
//! needs to decide which class a receiver denotes.

use crate::refs::util::{
    LocalBind, contains, locate_word_in, named_type, named_type_heads, type_name_symbol,
};
use crate::refs::{Resolved, Symbol};
use saule_ast::{Expr, Param, Spanned, Stmt, Type};
use saule_semantic::{lookup_field_type, with_classes, with_enums};
use std::ops::Range;

use super::*;

impl<'a> ResolveCx<'a> {
    pub(crate) fn record(&mut self, span: Range<usize>, symbol: Symbol) {
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

    pub(crate) fn lookup_local(&self, name: &str) -> Option<&LocalBind> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

    pub(crate) fn enter_function(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let saved = std::mem::take(&mut self.locals);
        self.push_params(params);
        body(self);
        self.locals = saved;
    }

    /// Walk into a lambda body *keeping* the enclosing scope and
    /// stacking the lambda's own parameters on top of it.
    ///
    /// A lambda is a closure, so every name around it is a name its body
    /// can use. Entering with a fresh scope — as this used to, sharing
    /// [`Self::enter_function`] — meant a captured local resolved to
    /// nothing and fell through to the "unknown identifier, must be a
    /// free function" branch, so goto-definition on it searched the
    /// whole workspace for a function that doesn't exist.
    pub(crate) fn enter_lambda(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let mark = self.locals.len();
        self.push_params(params);
        body(self);
        self.locals.truncate(mark);
    }

    fn push_params(&mut self, params: &[Param]) {
        for p in params {
            let def_span = locate_word_in(self.source, &p.span, &p.name).unwrap_or(p.span.clone());
            self.locals.push(LocalBind {
                name: p.name.clone(),
                def_span,
                ty: p.ty.clone(),
            });
        }
    }

    /// Record the parameter's own binding site plus any class-ish name
    /// in its type ascription, then walk its default expression.
    pub(crate) fn visit_param(&mut self, p: &Param) {
        if let Some(span) = locate_word_in(self.source, &p.span, &p.name) {
            self.record(
                span.clone(),
                Symbol::Local {
                    name: p.name.clone(),
                    def_span: span,
                },
            );
        }
        self.record_type_names_in(&p.ty, &p.span);
        if let Some(def) = &p.default {
            self.visit_expr(def);
        }
    }

    /// Make the class / interface / enum names written inside a type
    /// ascription navigable: `local p: Player` should reach `Player`'s
    /// declaration exactly like the `Player()` call on the same line
    /// does. `search` bounds where in the source the names are looked
    /// for (a parameter's span, a statement's head, …).
    pub(crate) fn record_type_names_in(&mut self, ty: &Type, search: &Range<usize>) {
        if !contains(search, self.offset) {
            return;
        }
        let mut names = Vec::new();
        named_type_heads(ty, &mut names);
        for name in names {
            let Some(span) = locate_word_in(self.source, search, &name) else {
                continue;
            };
            if !contains(&span, self.offset) {
                continue;
            }
            if let Some(sym) = type_name_symbol(&name) {
                self.record(span, sym);
            }
        }
    }

    /// Best-effort receiver type resolution, mirroring the hover
    /// receiver_class logic so member/method goto navigates to the
    /// right class.
    pub(crate) fn receiver_class(&self, obj: &Expr) -> Option<String> {
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
                crate::refs::util::method_call_class(&callee.value, |e| self.receiver_class(e))
            }
            _ => None,
        }
    }

    pub(crate) fn infer_local_ty(&self, init: &Expr) -> Type {
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
            _ => Type::Named("any".into()),
        }
    }

    pub(crate) fn push_local_binding(
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
        let span =
            locate_word_in(self.source, stmt_span, name).unwrap_or_else(|| stmt_span.clone());
        self.locals.push(LocalBind {
            name: name.to_string(),
            def_span: span,
            ty: resolved,
        });
    }

    pub(crate) fn push_locals_from_stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Local {
                name, ty, value, ..
            } => {
                self.push_local_binding(
                    name,
                    ty.clone(),
                    value.as_ref().map(|v| &v.value),
                    &s.span,
                );
            }
            Stmt::LocalMulti { names, values } => {
                for (i, (name, _, ty)) in names.iter().enumerate() {
                    let init = values.get(i).map(|v| &v.value);
                    self.push_local_binding(name, ty.clone(), init, &s.span);
                }
            }
            _ => {}
        }
    }
}
