//! Resolving declarations: functions, classes and methods, the
//! scopes they open, and the shape rules for variadic parameters.

use crate::error::SemanticError;
use crate::to_source_span;
use saule_ast::{ClassMember, Decl, Method, Param, Spanned, Stmt};

use super::*;

impl Resolver {
    pub(crate) fn decl(&mut self, decl: &Decl) {
        match decl {
            Decl::Function { params, body, .. } => {
                self.check_variadic_shape(params);
                self.enter_function(params, body);
            }
            Decl::Class { name, members, .. } => {
                let prev_class = self.in_class.replace(name.clone());

                // Inside a class's own methods, every static member is
                // reachable by its bare name (mirrors Lua-style `self` for
                // statics). Collect them once so each method body can be
                // walked with that set visible.
                let static_names: Vec<String> = members
                    .iter()
                    .filter_map(|m| match &m.value {
                        ClassMember::Field {
                            is_static: true,
                            name,
                            ..
                        } => Some(name.clone()),
                        ClassMember::Method(meth) if meth.is_static => Some(meth.name.clone()),
                        _ => None,
                    })
                    .collect();

                for m in members {
                    match &m.value {
                        ClassMember::Method(meth) => {
                            self.method(name, meth, &static_names);
                        }
                        ClassMember::Field {
                            default: Some(d), ..
                        } => self.expr(d),
                        ClassMember::Field { .. } => {}
                    }
                }
                self.in_class = prev_class;
            }
            Decl::Enum { methods, .. } => {
                for meth in methods {
                    self.check_variadic_shape(&meth.params);
                    let prev_method = std::mem::replace(&mut self.in_method, true);
                    self.enter_function(&meth.params, &meth.body);
                    self.in_method = prev_method;
                }
            }
            // Interface declarations carry only signatures; no body to walk.
            // Import declarations don't host expressions.
            Decl::Interface { .. } | Decl::Import { .. } => {}
        }
    }

    pub(crate) fn method(&mut self, class_name: &str, meth: &Method, static_names: &[String]) {
        self.check_variadic_shape(&meth.params);
        let prev_method = std::mem::replace(&mut self.in_method, true);
        let prev_init =
            std::mem::replace(&mut self.in_init, meth.name == "init" && !meth.is_static);

        // Default-value expressions evaluate in the *outer* scope, so walk
        // them before pushing the body frame.
        for p in &meth.params {
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }

        let prev_class = self.in_class.replace(class_name.to_string());
        self.push_scope();
        // Make the class's static members visible by bare name.
        for n in static_names {
            self.declare(n);
        }
        for p in &meth.params {
            self.declare(&p.name);
        }
        self.block(&meth.body);
        self.pop_scope();
        self.in_class = prev_class;

        self.in_init = prev_init;
        self.in_method = prev_method;
    }

    /// Push a fresh function-body scope, declare every param, walk the
    /// body, pop. Used for top-level `Decl::Function` and enum methods.
    pub(crate) fn enter_function(&mut self, params: &[Param], body: &[Spanned<Stmt>]) {
        // Default-value expressions are evaluated in the *outer* scope, so
        // walk them before pushing the body frame.
        for p in params {
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }
        self.push_scope();
        for p in params {
            self.declare(&p.name);
        }
        self.block(body);
        self.pop_scope();
    }

    pub(crate) fn check_variadic_shape(&mut self, params: &[Param]) {
        let variadic_positions: Vec<usize> = params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.variadic)
            .map(|(i, _)| i)
            .collect();

        if variadic_positions.len() > 1 {
            // Report every extra variadic individually.
            for idx in &variadic_positions[1..] {
                self.errors.push(SemanticError::MultipleVariadicParams {
                    span: to_source_span(params[*idx].span.clone()),
                });
            }
        }

        if let Some(&first_var) = variadic_positions.first()
            && first_var + 1 < params.len()
        {
            let p = &params[first_var];
            self.errors.push(SemanticError::VariadicNotLast {
                name: p.name.clone(),
                span: to_source_span(p.span.clone()),
            });
        }
    }

    // ── Expressions ────────────────────────────────────────────────────────
}
