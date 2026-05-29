//! Name resolution + a handful of small structural checks bundled into a
//! single AST walker so we don't traverse the tree N times.
//!
//! Emits:
//!
//! * [`SemanticError::UndefinedName`] — a bare ident that isn't in any
//!   lexical scope, isn't a top-level declaration of this module, isn't
//!   an imported name, and isn't in the prelude.
//! * [`SemanticError::AssignToUndeclared`] — an assignment whose LHS
//!   ident isn't in scope (the runtime previously caught these).
//! * [`SemanticError::SelfOutsideClass`] — `self` referenced outside any
//!   method body.
//! * [`SemanticError::SuperOutsideClass`] — `super.x` or `super(...)` used
//!   outside a method.
//! * [`SemanticError::SuperCallOutsideInit`] — `self.super(...)` outside
//!   `init`.
//! * [`SemanticError::MultipleVariadicParams`] /
//!   [`SemanticError::VariadicNotLast`] — declaration-time variadic shape.
//! * [`SemanticError::PositionalAfterNamed`] — argument-list ordering.
//! * [`SemanticError::ForInArity`] — `for v1, v2, v3 in iter` is invalid.
//!
//! ## Scoping
//!
//! Scopes are a stack of `HashSet<String>`. `Stmt::Local` binds into the
//! top frame. Every `if`/`while`/`repeat`/`for`/`try`/match-arm body
//! pushes its own frame so a `local` declared in a then-branch doesn't
//! leak to the else-branch (Lua-style block scoping).
//!
//! Functions, methods, and lambdas push a frame and reset the
//! enclosing-class / `in_init` flags appropriately. Module-level
//! declarations are pre-collected before the walk so forward references
//! to top-level `fn` / `class` / etc. resolve cleanly.
//!
//! ## Wildcard imports
//!
//! `import * from "..."` introduces names this pass cannot enumerate, so
//! when *any* wildcard import is seen the [`UndefinedName`] /
//! [`AssignToUndeclared`] checks become advisory: we still walk the AST
//! for the other diagnostics, but ident lookups that would otherwise fail
//! are silently accepted. Files without a wildcard import get the full
//! treatment.

use std::collections::HashSet;

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, ImportNames, LambdaBody, MatchBody, Method, Module, Param,
    Pattern, Spanned, Stmt, TableEntry,
};

use crate::error::SemanticError;
use crate::prelude;
use crate::to_source_span;

struct Resolver {
    /// Lexical scope stack. The bottom frame is the module scope (top-level
    /// decls + imported names); subsequent frames cover function/method
    /// bodies and blocks.
    scopes: Vec<HashSet<String>>,
    /// Class context for `self` / `super` validity. `None` at module scope.
    in_class: Option<String>,
    /// Are we currently inside the `init` constructor body of a class?
    in_init: bool,
    /// Walking inside any method body (including `init`). `self` is legal,
    /// `super.x` is legal (when a parent exists; we don't check that here),
    /// `self.super(...)` is only legal when also `in_init`.
    in_method: bool,
    /// True when the module contains `import * from "..."`. Suppresses
    /// undefined-name diagnostics since we can't enumerate the imported
    /// names statically.
    has_wildcard_import: bool,
    errors: Vec<SemanticError>,
}

pub(crate) fn check(module: &Module, errors: &mut Vec<SemanticError>) {
    let mut r = Resolver {
        scopes: vec![collect_module_scope(module)],
        in_class: None,
        in_init: false,
        in_method: false,
        has_wildcard_import: module_has_wildcard_import(module),
        errors: Vec::new(),
    };

    for s in &module.stmts {
        r.stmt(s);
    }

    errors.append(&mut r.errors);
}

fn module_has_wildcard_import(module: &Module) -> bool {
    module.stmts.iter().any(|s| {
        matches!(
            &s.value,
            Stmt::Decl(d) if matches!(&d.value, Decl::Import { names: ImportNames::All, .. })
        )
    })
}

/// Pre-collect every name visible at module scope so forward references
/// (e.g. `fn a() b() end` then `fn b() end`) resolve cleanly.
fn collect_module_scope(module: &Module) -> HashSet<String> {
    let mut scope: HashSet<String> = HashSet::new();
    for s in &module.stmts {
        match &s.value {
            Stmt::Local { name, .. } => {
                scope.insert(name.clone());
            }
            Stmt::LocalMulti { names, .. } => {
                for (n, _) in names {
                    scope.insert(n.clone());
                }
            }
            Stmt::Decl(d) => match &d.value {
                Decl::Function { name, .. }
                | Decl::Class { name, .. }
                | Decl::Interface { name, .. }
                | Decl::Enum { name, .. } => {
                    scope.insert(name.clone());
                }
                Decl::Import { names, .. } => match names {
                    ImportNames::All => {}
                    ImportNames::List(items) => {
                        for (orig, alias) in items {
                            scope.insert(alias.clone().unwrap_or_else(|| orig.clone()));
                        }
                    }
                },
            },
            _ => {}
        }
    }
    scope
}

impl Resolver {
    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    fn declare(&mut self, name: &str) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name.to_string());
        }
    }
    fn resolved(&self, name: &str) -> bool {
        if self.has_wildcard_import {
            return true;
        }
        if self.scopes.iter().any(|f| f.contains(name)) {
            return true;
        }
        prelude::contains(name)
    }

    // ── Statements ─────────────────────────────────────────────────────────

    fn stmt(&mut self, stmt: &Spanned<Stmt>) {
        match &stmt.value {
            Stmt::Local { name, value, .. } => {
                if let Some(v) = value {
                    self.expr(v);
                }
                self.declare(name);
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.expr(v);
                }
                for (n, _) in names {
                    self.declare(n);
                }
            }
            Stmt::Assign { target, value } => {
                if let Expr::Ident(name) = &target.value
                    && !self.resolved(name)
                {
                    self.errors.push(SemanticError::AssignToUndeclared {
                        name: name.clone(),
                        span: to_source_span(target.span.clone()),
                    });
                } else {
                    // Targets other than plain idents go through `expr`
                    // for member/index resolution.
                    if !matches!(target.value, Expr::Ident(_)) {
                        self.expr(target);
                    }
                }
                self.expr(value);
            }
            Stmt::AssignMulti { targets, values } => {
                for v in values {
                    self.expr(v);
                }
                for t in targets {
                    if let Expr::Ident(name) = &t.value {
                        if !self.resolved(name) {
                            self.errors.push(SemanticError::AssignToUndeclared {
                                name: name.clone(),
                                span: to_source_span(t.span.clone()),
                            });
                        }
                    } else {
                        self.expr(t);
                    }
                }
            }
            Stmt::Expr(e) | Stmt::Throw(e) => self.expr(e),

            Stmt::If { cond, then_block, elseifs, else_block } => {
                self.expr(cond);
                self.push_scope();
                self.block(then_block);
                self.pop_scope();
                for (c, b) in elseifs {
                    self.expr(c);
                    self.push_scope();
                    self.block(b);
                    self.pop_scope();
                }
                if let Some(b) = else_block {
                    self.push_scope();
                    self.block(b);
                    self.pop_scope();
                }
            }
            Stmt::While { cond, body } => {
                self.expr(cond);
                self.push_scope();
                self.block(body);
                self.pop_scope();
            }
            Stmt::Repeat { body, cond } => {
                // Lua-style: `until` cond sees locals declared in body, so
                // walk the cond *before* popping.
                self.push_scope();
                self.block(body);
                self.expr(cond);
                self.pop_scope();
            }
            Stmt::ForNumeric { var, from, to, step, body, .. } => {
                self.expr(from);
                self.expr(to);
                if let Some(s) = step {
                    self.expr(s);
                }
                self.push_scope();
                self.declare(var);
                self.block(body);
                self.pop_scope();
            }
            Stmt::ForIn { vars, iter, body } => {
                if vars.is_empty() || vars.len() > 2 {
                    self.errors.push(SemanticError::ForInArity {
                        found: vars.len(),
                        span: to_source_span(stmt.span.clone()),
                    });
                }
                self.expr(iter);
                self.push_scope();
                for (n, _) in vars {
                    self.declare(n);
                }
                self.block(body);
                self.pop_scope();
            }
            Stmt::Return(values) => {
                for v in values {
                    self.expr(v);
                }
            }
            Stmt::Try { body, catch_var, catch_body, .. } => {
                self.push_scope();
                self.block(body);
                self.pop_scope();
                self.push_scope();
                self.declare(catch_var);
                self.block(catch_body);
                self.pop_scope();
            }
            Stmt::Break | Stmt::Continue => {}

            Stmt::Decl(d) => self.decl(&d.value),
        }
    }

    fn block(&mut self, body: &[Spanned<Stmt>]) {
        for s in body {
            self.stmt(s);
        }
    }

    // ── Declarations ───────────────────────────────────────────────────────

    fn decl(&mut self, decl: &Decl) {
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
                        ClassMember::Field { is_static: true, name, .. } => Some(name.clone()),
                        ClassMember::Method(meth) if meth.is_static => Some(meth.name.clone()),
                        _ => None,
                    })
                    .collect();

                for m in members {
                    match &m.value {
                        ClassMember::Method(meth) => {
                            self.method(name, meth, &static_names);
                        }
                        ClassMember::Field { default: Some(d), .. } => self.expr(d),
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

    fn method(&mut self, class_name: &str, meth: &Method, static_names: &[String]) {
        self.check_variadic_shape(&meth.params);
        let prev_method = std::mem::replace(&mut self.in_method, true);
        let prev_init = std::mem::replace(&mut self.in_init, meth.name == "init" && !meth.is_static);

        // Default-value expressions evaluate in the *outer* scope, so walk
        // them before pushing the body frame.
        for p in &meth.params {
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }

        let prev_class = std::mem::replace(&mut self.in_class, Some(class_name.to_string()));
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
    fn enter_function(&mut self, params: &[Param], body: &[Spanned<Stmt>]) {
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

    fn check_variadic_shape(&mut self, params: &[Param]) {
        let variadic_positions: Vec<usize> =
            params.iter().enumerate().filter(|(_, p)| p.variadic).map(|(i, _)| i).collect();

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

    fn expr(&mut self, expr: &Spanned<Expr>) {
        let span = expr.span.clone();
        match &expr.value {
            Expr::Ident(name) => {
                if !self.resolved(name) {
                    self.errors.push(SemanticError::UndefinedName {
                        name: name.clone(),
                        span: to_source_span(span),
                    });
                }
            }
            Expr::Self_ => {
                if !self.in_method {
                    self.errors.push(SemanticError::SelfOutsideClass {
                        span: to_source_span(span),
                    });
                }
            }
            Expr::Unary { rhs, .. } => self.expr(rhs),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Member { obj, name } => {
                self.check_super_receiver(obj, name, &span);
                self.expr(obj);
            }
            Expr::SafeMember { obj, .. } => self.expr(obj),
            Expr::Index { obj, index } => {
                self.expr(obj);
                self.expr(index);
            }
            Expr::Call { callee, args } => {
                self.check_arg_ordering(args);
                self.check_super_call(callee, &span);
                self.expr(callee);
                for a in args {
                    self.call_arg(a);
                }
            }
            Expr::MethodCall { obj, args, .. } => {
                self.check_arg_ordering(args);
                self.expr(obj);
                for a in args {
                    self.call_arg(a);
                }
            }
            Expr::ForceUnwrap(inner) => self.expr(inner),
            Expr::Table(entries) => {
                for e in entries {
                    match e {
                        TableEntry::Positional(v) => self.expr(v),
                        TableEntry::Field { key, value } => {
                            self.expr(key);
                            self.expr(value);
                        }
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                self.check_variadic_shape(params);
                // Default exprs evaluated in outer scope.
                for p in params {
                    if let Some(d) = &p.default {
                        self.expr(d);
                    }
                }
                self.push_scope();
                for p in params {
                    self.declare(&p.name);
                }
                match body {
                    LambdaBody::Expr(e) => self.expr(e),
                    LambdaBody::Block(b) => self.block(b),
                }
                self.pop_scope();
            }
            Expr::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.push_scope();
                    self.bind_pattern(&arm.pattern.value);
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    match &arm.body {
                        MatchBody::Expr(e) => self.expr(e),
                        MatchBody::Block(b) => self.block(b),
                    }
                    self.pop_scope();
                }
            }
            // `when(source):stage1(args):stage2(args)…` — resolve the
            // source like a normal expression, then treat every stage
            // function name as an identifier lookup at its own span so
            // typos surface as `UndefinedName`. Stage args are checked
            // for ordering (positionals before named) just like a regular
            // call.
            Expr::Pipe { source, stages } => {
                self.expr(source);
                for stage in stages {
                    self.check_arg_ordering(&stage.args);
                    if !self.resolved(&stage.name) {
                        self.errors.push(SemanticError::UndefinedName {
                            name: stage.name.clone(),
                            span: to_source_span(stage.span.clone()),
                        });
                    }
                    for a in &stage.args {
                        self.call_arg(a);
                    }
                }
            }
            // Pure literals — nothing to resolve.
            Expr::Int(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Str(_) | Expr::Nil => {}
        }
    }

    fn call_arg(&mut self, a: &CallArg) {
        match a {
            CallArg::Positional(e) | CallArg::Named { value: e, .. } => self.expr(e),
        }
    }

    fn check_arg_ordering(&mut self, args: &[CallArg]) {
        let mut seen_named = false;
        for a in args {
            match a {
                CallArg::Named { .. } => seen_named = true,
                CallArg::Positional(e) if seen_named => {
                    self.errors.push(SemanticError::PositionalAfterNamed {
                        span: to_source_span(e.span.clone()),
                    });
                    break;
                }
                CallArg::Positional(_) => {}
            }
        }
    }

    /// Catch `super.x` outside a method (otherwise valid super uses are
    /// further validated when paired with a Call in `check_super_call`).
    fn check_super_receiver(
        &mut self,
        obj: &Spanned<Expr>,
        _name: &str,
        span: &std::ops::Range<usize>,
    ) {
        if let Expr::Ident(n) = &obj.value
            && n == "super"
            && !self.in_method
        {
            self.errors.push(SemanticError::SuperOutsideClass {
                span: to_source_span(span.clone()),
            });
        }
    }

    /// `self.super(...)` parses as `Call { callee: Member { obj: Self_, name: "super" }, .. }`.
    /// Only legal inside an `init` body of a subclass — we check the
    /// `in_init` flag here; the "subclass" half is left to the runtime
    /// since this pass doesn't know parent-class info.
    fn check_super_call(&mut self, callee: &Spanned<Expr>, span: &std::ops::Range<usize>) {
        if let Expr::Member { obj, name } = &callee.value
            && name == "super"
            && matches!(obj.value, Expr::Self_)
            && !self.in_init
        {
            self.errors.push(SemanticError::SuperCallOutsideInit {
                span: to_source_span(span.clone()),
            });
        }
    }

    // ── Patterns ───────────────────────────────────────────────────────────

    fn bind_pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Bind(name) => self.declare(name),
            Pattern::Variant { fields, .. } => {
                for sub in fields {
                    self.bind_pattern(&sub.value);
                }
            }
            Pattern::Tuple(elems) => {
                for sub in elems {
                    self.bind_pattern(&sub.value);
                }
            }
            // Wildcards & literals bind nothing.
            Pattern::Wildcard
            | Pattern::Nil
            | Pattern::Int(_)
            | Pattern::Float(_)
            | Pattern::Bool(_)
            | Pattern::Str(_) => {}
        }
    }
}

