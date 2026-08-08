//! Local scope tracking and the lightweight type inference the walk
//! needs to decide which class a receiver denotes.

use crate::refs::util::{LocalBind, locate_word_in, named_type, named_type_heads};
use crate::refs::{Hit, Symbol};
use saule_ast::{Expr, Param, Type};
use saule_semantic::{lookup_field_type, with_classes, with_enums};
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

    pub(crate) fn push(&mut self, span: Range<usize>, is_def: bool) {
        self.out.push(Hit { span, is_def });
    }

    pub(crate) fn enter_function(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let saved = std::mem::take(&mut self.locals);
        self.push_params(params);
        body(self);
        self.locals = saved;
    }

    /// Lambda bodies keep the enclosing scope — see the matching
    /// `ResolveCx::enter_lambda`, whose bindings this must agree with
    /// for a captured local's references to be found at all.
    pub(crate) fn enter_lambda(&mut self, params: &[Param], body: impl FnOnce(&mut Self)) {
        let mark = self.locals.len();
        self.push_params(params);
        body(self);
        self.locals.truncate(mark);
    }

    fn push_params(&mut self, params: &[Param]) {
        for p in params {
            let def_span = locate_word_in(self.source, &p.span, &p.name).unwrap_or(p.span.clone());
            // Param binding sites aren't a Local "definition" in the
            // referencing-search sense unless the target Symbol is
            // exactly this binding.
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
    }

    /// Emit a reference for every mention of the target class /
    /// interface / enum inside a type ascription found in `search`.
    pub(crate) fn collect_type_name_refs_in(&mut self, ty: &Type, search: &Range<usize>) {
        let target = match self.symbol {
            Symbol::Class(n) | Symbol::Interface(n) | Symbol::Enum(n) => n.clone(),
            _ => return,
        };
        let mut names = Vec::new();
        named_type_heads(ty, &mut names);
        if !names.iter().any(|n| n == &target) {
            return;
        }
        if let Some(span) = locate_word_in(self.source, search, &target) {
            self.push(span, false);
        }
    }
}
