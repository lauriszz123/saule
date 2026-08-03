//! Local scope tracking and the lightweight type inference the walk
//! needs to decide which class a receiver denotes.

use crate::refs::util::{LocalBind, contains, locate_word_in, named_type};
use crate::refs::{Resolved, Symbol};
use saule_ast::{Expr, Param, Spanned, Stmt, Type};
use saule_semantic::{lookup_field_type, lookup_method, with_classes, with_enums};
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
        for p in params {
            let def_span = locate_word_in(self.source, &p.span, &p.name).unwrap_or(p.span.clone());
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
                None
            }
            Expr::MethodCall {
                obj: inner, method, ..
            } => {
                let inner_class = self.receiver_class(&inner.value)?;
                let sig = lookup_method(&inner_class, method)?;
                named_type(sig.return_ty.as_ref()?)
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
