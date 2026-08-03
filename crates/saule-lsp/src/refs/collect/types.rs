//! Local scope tracking and the lightweight type inference the walk
//! needs to decide which class a receiver denotes.

use crate::refs::util::{LocalBind, locate_word_in, named_type};
use crate::refs::{Hit, Symbol};
use saule_ast::{Expr, Param, Type};
use saule_semantic::{lookup_field_type, lookup_method, with_classes, with_enums};
use std::ops::Range;

use super::*;

impl<'a> CollectCx<'a> {
    pub(crate) fn lookup_local(&self, name: &str) -> Option<&LocalBind> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

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

    pub(crate) fn push(&mut self, span: Range<usize>, is_def: bool) {
        self.out.push(Hit { span, is_def });
    }

    pub(crate) fn enter_function(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
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
}
