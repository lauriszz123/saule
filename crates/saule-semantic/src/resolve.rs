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
//! `import * from "..."` introduces names this crate can't enumerate on
//! its own — it has no module loader. The embedder resolves them and
//! hands the result over as [`crate::ModuleSeed::wildcard_names`]:
//!
//! * `Some(names)` — every wildcard target was enumerated. The names go
//!   into the module scope and the [`UndefinedName`] /
//!   [`AssignToUndeclared`] checks stay fully active, so a typo still
//!   gets reported in a file that globs a module.
//! * `None` — at least one target couldn't be enumerated (or the
//!   embedder doesn't resolve imports at all). Those two checks then
//!   become advisory for any module containing a wildcard import: we
//!   still walk the AST for the other diagnostics, but ident lookups
//!   that would otherwise fail are silently accepted.

mod decls;
mod exprs;

mod scope;

pub(crate) use scope::*;

use std::collections::HashSet;

use saule_ast::{Expr, Module, Spanned, Stmt};

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
    /// True when the module contains `import * from "..."` *and* the
    /// embedder couldn't tell us what those imports bind. Suppresses
    /// undefined-name diagnostics, since any unknown ident might have
    /// come in through the glob.
    has_opaque_wildcard_import: bool,
    errors: Vec<SemanticError>,
}

pub(crate) fn check(
    module: &Module,
    wildcard_names: Option<&HashSet<String>>,
    errors: &mut Vec<SemanticError>,
) {
    let mut module_scope = collect_module_scope(module);
    if let Some(names) = wildcard_names {
        module_scope.extend(names.iter().cloned());
    }

    let mut r = Resolver {
        scopes: vec![module_scope],
        in_class: None,
        in_init: false,
        in_method: false,
        has_opaque_wildcard_import: wildcard_names.is_none() && module_has_wildcard_import(module),
        errors: Vec::new(),
    };

    for s in &module.stmts {
        r.stmt(s);
    }

    errors.append(&mut r.errors);
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
        if self.has_opaque_wildcard_import {
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
                for (n, _, _) in names {
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

            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
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
            Stmt::ForNumeric {
                var,
                from,
                to,
                step,
                body,
                ..
            } => {
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
            Stmt::Try {
                body,
                catch_var,
                catch_body,
                ..
            } => {
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
}
