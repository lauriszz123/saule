//! Cursor-position resolver — walks the AST and identifies what
//! semantic symbol the byte offset is on. Used by goto-definition.

mod decls;
mod exprs;
mod types;

use saule_ast::{Module, Spanned, Stmt, Type};

use super::util::{LocalBind, catch_var_span, contains, locate_word_in};
use super::{Resolved, Symbol};

pub(super) fn run(module: &Module, source: &str, offset: usize) -> Option<Resolved> {
    let mut cx = ResolveCx {
        offset,
        source,
        enclosing_class: None,
        locals: Vec::new(),
        best: None,
    };
    cx.visit_module(module);
    cx.best
}

struct ResolveCx<'a> {
    offset: usize,
    source: &'a str,
    enclosing_class: Option<String>,
    locals: Vec<LocalBind>,
    best: Option<Resolved>,
}

impl<'a> ResolveCx<'a> {
    fn visit_module(&mut self, m: &Module) {
        for s in &m.stmts {
            self.visit_stmt(s);
        }
    }

    fn visit_stmt(&mut self, s: &Spanned<Stmt>) {
        if !contains(&s.span, self.offset) && !matches!(&s.value, Stmt::Decl(_)) {
            // Locals declared in this stmt still need to be pushed
            // even when the cursor isn't in it, so following stmts
            // resolve correctly. Drop only when we're sure we're past.
            if s.span.end < self.offset {
                self.push_locals_from_stmt(s);
            }
            return;
        }
        match &s.value {
            // A recovery hole has no children to walk.
            Stmt::Error => {}
            Stmt::Decl(d) => self.visit_decl(d),
            Stmt::Local {
                name, ty, value, ..
            } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                if let Some(t) = ty {
                    // Bound to the head so a class name spelled in both
                    // the ascription and the initialiser (`local p:
                    // Player = Player()`) picks the ascription.
                    let head_end = value.as_ref().map(|v| v.span.start).unwrap_or(s.span.end);
                    self.record_type_names_in(t, &(s.span.start..head_end));
                }
                if let Some(span) = locate_word_in(self.source, &s.span, name) {
                    self.record(
                        span.clone(),
                        Symbol::Local {
                            name: name.clone(),
                            def_span: span,
                        },
                    );
                }
                self.push_local_binding(
                    name,
                    ty.clone(),
                    value.as_ref().map(|v| &v.value),
                    &s.span,
                );
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.visit_expr(v);
                }
                for (i, (name, name_span, ty)) in names.iter().enumerate() {
                    self.record(
                        name_span.clone(),
                        Symbol::Local {
                            name: name.clone(),
                            def_span: name_span.clone(),
                        },
                    );
                    if let Some(t) = ty {
                        // Each name's ascription lives between it and
                        // the next name.
                        let end = names
                            .get(i + 1)
                            .map(|(_, next, _)| next.start)
                            .unwrap_or(s.span.end);
                        self.record_type_names_in(t, &(name_span.end..end));
                    }
                    let init = values.get(i).map(|v| &v.value);
                    self.push_local_binding(name, ty.clone(), init, &s.span);
                }
            }
            Stmt::Assign { target, value } | Stmt::CompoundAssign { target, value, .. } => {
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
                self.with_block(|this| {
                    for s in then_block {
                        this.visit_stmt(s);
                    }
                });
                for (c, b) in elseifs {
                    self.visit_expr(c);
                    self.with_block(|this| {
                        for s in b {
                            this.visit_stmt(s);
                        }
                    });
                }
                if let Some(eb) = else_block {
                    self.with_block(|this| {
                        for s in eb {
                            this.visit_stmt(s);
                        }
                    });
                }
            }
            Stmt::While { cond, body } => {
                self.visit_expr(cond);
                self.with_block(|this| {
                    for s in body {
                        this.visit_stmt(s);
                    }
                });
            }
            Stmt::Repeat { body, cond } => {
                self.with_block(|this| {
                    for s in body {
                        this.visit_stmt(s);
                    }
                });
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
                if let Some(st) = step {
                    self.visit_expr(st);
                }
                let span =
                    locate_word_in(self.source, &s.span, var).unwrap_or_else(|| s.span.clone());
                self.record(
                    span.clone(),
                    Symbol::Local {
                        name: var.clone(),
                        def_span: span.clone(),
                    },
                );
                if let Some(t) = var_ty {
                    self.record_type_names_in(t, &(span.end..from.span.start));
                }
                let saved = self.locals.len();
                self.locals.push(LocalBind {
                    name: var.clone(),
                    def_span: span,
                    ty: var_ty
                        .clone()
                        .unwrap_or_else(|| Type::Named("integer".into())),
                });
                for s in body {
                    self.visit_stmt(s);
                }
                self.locals.truncate(saved);
            }
            Stmt::ForIn { vars, iter, body } => {
                self.visit_expr(iter);
                let saved = self.locals.len();
                for (name, ty) in vars {
                    let span = locate_word_in(self.source, &s.span, name)
                        .unwrap_or_else(|| s.span.clone());
                    self.record(
                        span.clone(),
                        Symbol::Local {
                            name: name.clone(),
                            def_span: span.clone(),
                        },
                    );
                    if let Some(t) = ty {
                        self.record_type_names_in(t, &(span.end..iter.span.start));
                    }
                    self.locals.push(LocalBind {
                        name: name.clone(),
                        def_span: span,
                        ty: ty.clone().unwrap_or_else(|| Type::Named("any".into())),
                    });
                }
                for s in body {
                    self.visit_stmt(s);
                }
                self.locals.truncate(saved);
            }
            Stmt::Return(es) => {
                for e in es {
                    self.visit_expr(e);
                }
            }
            Stmt::Throw(e) => self.visit_expr(e),
            Stmt::Try {
                body,
                catch_var,
                catch_ty,
                catch_body,
            } => {
                self.with_block(|this| {
                    for s in body {
                        this.visit_stmt(s);
                    }
                });
                let span = catch_var_span(self.source, &s.span, body, catch_var);
                self.record(
                    span.clone(),
                    Symbol::Local {
                        name: catch_var.clone(),
                        def_span: span.clone(),
                    },
                );
                let ty_end = catch_body
                    .first()
                    .map(|c| c.span.start)
                    .unwrap_or(s.span.end);
                self.record_type_names_in(catch_ty, &(span.end..ty_end));
                let saved = self.locals.len();
                self.locals.push(LocalBind {
                    name: catch_var.clone(),
                    def_span: span,
                    ty: catch_ty.clone(),
                });
                for s in catch_body {
                    self.visit_stmt(s);
                }
                self.locals.truncate(saved);
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn with_block(&mut self, body: impl FnOnce(&mut Self)) {
        let saved = self.locals.len();
        body(self);
        self.locals.truncate(saved);
    }
}
