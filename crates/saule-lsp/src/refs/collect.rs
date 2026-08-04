//! Reference collector — walks the AST and emits every span that
//! defines or references a given [`Symbol`]. Used by find-references.

mod decls;
mod exprs;
mod types;

use saule_ast::{Module, Spanned, Stmt, Type};

use super::util::{LocalBind, catch_var_span, locate_word_in};
use super::{Hit, Symbol};

pub(super) fn run(module: &Module, source: &str, symbol: &Symbol) -> Vec<Hit> {
    let mut cx = CollectCx {
        source,
        symbol,
        enclosing_class: None,
        locals: Vec::new(),
        out: Vec::new(),
    };
    cx.visit_module(module);
    cx.out
}

struct CollectCx<'a> {
    source: &'a str,
    symbol: &'a Symbol,
    enclosing_class: Option<String>,
    locals: Vec<LocalBind>,
    out: Vec<Hit>,
}

impl<'a> CollectCx<'a> {
    fn visit_module(&mut self, m: &Module) {
        for s in &m.stmts {
            self.visit_stmt(s);
        }
    }

    fn visit_stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Decl(d) => self.visit_decl(d),
            Stmt::Local {
                name, ty, value, ..
            } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                if let Some(t) = ty {
                    let head_end = value.as_ref().map(|v| v.span.start).unwrap_or(s.span.end);
                    self.collect_type_name_refs_in(t, &(s.span.start..head_end));
                }
                let def_span = locate_word_in(self.source, &s.span, name);
                if let Some(span) = &def_span
                    && let Symbol::Local {
                        name: tname,
                        def_span: tspan,
                    } = self.symbol
                    && tname == name
                    && tspan == span
                {
                    self.push(span.clone(), true);
                }
                let resolved = ty.clone().unwrap_or_else(|| match value {
                    Some(v) => self.infer_local_ty(&v.value),
                    None => Type::Named("any".into()),
                });
                self.locals.push(LocalBind {
                    name: name.clone(),
                    def_span: def_span.unwrap_or_else(|| s.span.clone()),
                    ty: resolved,
                });
            }
            Stmt::LocalMulti { names, values } => {
                for v in values {
                    self.visit_expr(v);
                }
                for (i, (name, name_span, ty)) in names.iter().enumerate() {
                    if let Symbol::Local {
                        name: tname,
                        def_span: tspan,
                    } = self.symbol
                        && tname == name
                        && tspan == name_span
                    {
                        self.push(name_span.clone(), true);
                    }
                    if let Some(t) = ty {
                        let end = names
                            .get(i + 1)
                            .map(|(_, next, _)| next.start)
                            .unwrap_or(s.span.end);
                        self.collect_type_name_refs_in(t, &(name_span.end..end));
                    }
                    let resolved = ty.clone().unwrap_or_else(|| match values.get(i) {
                        Some(v) => self.infer_local_ty(&v.value),
                        None => Type::Named("any".into()),
                    });
                    self.locals.push(LocalBind {
                        name: name.clone(),
                        def_span: name_span.clone(),
                        ty: resolved,
                    });
                }
            }
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
                if let Symbol::Local { name, def_span } = self.symbol
                    && name == var
                    && def_span == &span
                {
                    self.push(span.clone(), true);
                }
                if let Some(t) = var_ty {
                    self.collect_type_name_refs_in(t, &(span.end..from.span.start));
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
                    if let Symbol::Local {
                        name: tname,
                        def_span,
                    } = self.symbol
                        && tname == name
                        && def_span == &span
                    {
                        self.push(span.clone(), true);
                    }
                    if let Some(t) = ty {
                        self.collect_type_name_refs_in(t, &(span.end..iter.span.start));
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
                if let Symbol::Local { name, def_span } = self.symbol
                    && name == catch_var
                    && def_span == &span
                {
                    self.push(span.clone(), true);
                }
                let ty_end = catch_body
                    .first()
                    .map(|c| c.span.start)
                    .unwrap_or(s.span.end);
                self.collect_type_name_refs_in(catch_ty, &(span.end..ty_end));
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

// ──────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ──────────────────────────────────────────────────────────────────────────────
