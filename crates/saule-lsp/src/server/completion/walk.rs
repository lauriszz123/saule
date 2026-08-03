//! Locating the cursor and deciding what kind of completion the
//! position calls for (member access, type position, import name,
//! or a bare identifier).

use saule_ast::{
    CallArg, ClassMember, Decl, Expr, LambdaBody, MatchBody, Module, Param, Spanned, Stmt, Type,
};

use super::*;

/// A binding visible at the caret.
pub(crate) struct Visible {
    pub(crate) name: String,
    /// Declared type, when the binding was annotated.
    pub(crate) ty: Option<Type>,
    /// Initialiser expression, kept so an un-annotated `local p = Player(…)`
    /// can still be resolved — type annotations are optional in Saule, so
    /// relying on `ty` alone would miss most real code.
    pub(crate) init: Option<Expr>,
    pub(crate) kind: &'static str,
}

pub(crate) enum Ctx {
    /// `receiver.<caret>` — the receiver expression.
    Member(Spanned<Expr>),
    /// A type annotation or return type.
    TypeName,
    /// `class Foo extends <caret>` — a base class. `exclude` holds the names
    /// that would close a cycle in the inheritance chain.
    BaseClass { exclude: Vec<String> },
    /// `class Foo implements <caret>` or `interface Foo extends <caret>` —
    /// interfaces, minus the ones already named in the header.
    Interfaces { exclude: Vec<String> },
    /// A value position. `stmt_start` is true when the caret begins a
    /// statement, which is the only place statement keywords make sense.
    Value { stmt_start: bool },
    /// `import * from <caret>` — offer importable modules. `quoted` mirrors
    /// how the author spelled the path so suggestions match their style.
    ImportPath { quoted: bool },
    /// `import <caret> from some.module` — offer that module's exports.
    ImportName { path: String },
}

pub(crate) struct Found {
    pub(crate) ctx: Ctx,
    pub(crate) scope: Vec<Visible>,
    pub(crate) class: Option<String>,
}

/// Descends the tree to the sentinel, maintaining the scope stack.
pub(crate) struct Walk {
    scope: Vec<Visible>,
    class: Option<String>,
    found: Option<Found>,
}

impl Walk {
    pub(crate) fn run(module: &Module) -> Option<Found> {
        let mut w = Walk {
            scope: Vec::new(),
            class: None,
            found: None,
        };
        w.block(&module.stmts);
        w.found
    }

    fn record(&mut self, ctx: Ctx) {
        if self.found.is_none() {
            self.found = Some(Found {
                ctx,
                scope: self
                    .scope
                    .iter()
                    .map(|v| Visible {
                        name: v.name.clone(),
                        ty: v.ty.clone(),
                        init: v.init.clone(),
                        kind: v.kind,
                    })
                    .collect(),
                class: self.class.clone(),
            });
        }
    }

    fn bind(&mut self, name: &str, ty: Option<Type>, kind: &'static str) {
        self.bind_init(name, ty, None, kind);
    }

    fn bind_init(&mut self, name: &str, ty: Option<Type>, init: Option<Expr>, kind: &'static str) {
        if name != SENTINEL {
            self.scope.push(Visible {
                name: name.to_string(),
                ty,
                init,
                kind,
            });
        }
    }

    /// Walk a block, adding each statement's bindings only *after* visiting it
    /// — so a `local` is not visible inside its own initialiser, and names
    /// declared later in the block are not offered at the caret.
    fn block(&mut self, stmts: &[Spanned<Stmt>]) {
        let mark = self.scope.len();
        for s in stmts {
            // A bare sentinel statement means the caret starts a statement.
            if let Stmt::Expr(e) = &s.value
                && matches!(&e.value, Expr::Ident(n) if n == SENTINEL)
            {
                self.record(Ctx::Value { stmt_start: true });
                continue;
            }
            self.stmt(&s.value);
            self.declare(&s.value);
        }
        self.scope.truncate(mark);
    }

    /// Add the bindings a statement introduces into the enclosing block.
    fn declare(&mut self, s: &Stmt) {
        match s {
            Stmt::Local {
                name, ty, value, ..
            } => self.bind_init(
                name,
                ty.clone(),
                value.as_ref().map(|v| v.value.clone()),
                "local",
            ),
            Stmt::LocalMulti { names, values } => {
                // Only pair names with values when the arity matches; a
                // multi-return call feeding several names is not 1:1.
                let paired = names.len() == values.len();
                for (i, (n, _, ty)) in names.iter().enumerate() {
                    let init = paired.then(|| values[i].value.clone());
                    self.bind_init(n, ty.clone(), init, "local");
                }
            }
            _ => {}
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Local { ty, value, .. } => {
                self.ty(ty.as_ref());
                self.opt_expr(value.as_ref());
            }
            Stmt::LocalMulti { names, values } => {
                for (_, _, ty) in names {
                    self.ty(ty.as_ref());
                }
                self.exprs(values);
            }
            Stmt::Assign { target, value } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::AssignMulti { targets, values } => {
                self.exprs(targets);
                self.exprs(values);
            }
            Stmt::Expr(e) => self.expr(e),
            Stmt::Return(es) => self.exprs(es),
            Stmt::Throw(e) => self.expr(e),
            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
                self.expr(cond);
                self.block(then_block);
                for (c, b) in elseifs {
                    self.expr(c);
                    self.block(b);
                }
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            Stmt::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::Repeat { body, cond } => {
                self.block(body);
                self.expr(cond);
            }
            Stmt::ForNumeric {
                var,
                var_ty,
                from,
                to,
                step,
                body,
            } => {
                self.ty(var_ty.as_ref());
                self.expr(from);
                self.expr(to);
                self.opt_expr(step.as_ref());
                let mark = self.scope.len();
                self.bind(var, var_ty.clone(), "loop variable");
                self.block(body);
                self.scope.truncate(mark);
            }
            Stmt::ForIn { vars, iter, body } => {
                for (_, ty) in vars {
                    self.ty(ty.as_ref());
                }
                self.expr(iter);
                let mark = self.scope.len();
                for (n, ty) in vars {
                    self.bind(n, ty.clone(), "loop variable");
                }
                self.block(body);
                self.scope.truncate(mark);
            }
            Stmt::Try {
                body,
                catch_var,
                catch_ty,
                catch_body,
            } => {
                self.block(body);
                self.ty(Some(catch_ty));
                let mark = self.scope.len();
                self.bind(catch_var, Some(catch_ty.clone()), "caught error");
                self.block(catch_body);
                self.scope.truncate(mark);
            }
            Stmt::Decl(d) => self.decl(&d.value),
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn decl(&mut self, d: &Decl) {
        match d {
            Decl::Function {
                params,
                return_ty,
                body,
                ..
            } => {
                self.params(params);
                self.ty(return_ty.as_ref());
                let mark = self.scope.len();
                for p in params {
                    self.bind(&p.name, Some(p.ty.clone()), "parameter");
                }
                self.block(body);
                self.scope.truncate(mark);
            }
            Decl::Class {
                name,
                extends,
                implements,
                members,
                ..
            } => {
                if extends.as_deref() == Some(SENTINEL) {
                    self.record(Ctx::BaseClass {
                        exclude: vec![name.clone()],
                    });
                }
                if implements.iter().any(|i| i == SENTINEL) {
                    self.record(Ctx::Interfaces {
                        exclude: without_sentinel(implements),
                    });
                }
                let prev = self.class.replace(name.clone());
                for m in members {
                    match &m.value {
                        ClassMember::Field { ty, default, .. } => {
                            self.ty(Some(ty));
                            self.opt_expr(default.as_ref());
                        }
                        ClassMember::Method(method) => {
                            self.params(&method.params);
                            self.ty(method.return_ty.as_ref());
                            let mark = self.scope.len();
                            for p in &method.params {
                                self.bind(&p.name, Some(p.ty.clone()), "parameter");
                            }
                            self.block(&method.body);
                            self.scope.truncate(mark);
                        }
                    }
                }
                self.class = prev;
            }
            Decl::Interface {
                name,
                extends,
                methods,
                ..
            } => {
                if extends.iter().any(|e| e == SENTINEL) {
                    let mut exclude = without_sentinel(extends);
                    exclude.push(name.clone());
                    self.record(Ctx::Interfaces { exclude });
                }
                for m in methods {
                    self.params(&m.params);
                    self.ty(m.return_ty.as_ref());
                }
            }
            Decl::Enum { methods, .. } => {
                for m in methods {
                    self.params(&m.params);
                    self.ty(m.return_ty.as_ref());
                    self.block(&m.body);
                }
            }
            Decl::Import {
                names,
                path,
                quoted,
            } => {
                // The sentinel lands inside the path (bare or quoted) when the
                // caret is choosing a module...
                if path.contains(SENTINEL) {
                    self.record(Ctx::ImportPath { quoted: *quoted });
                } else if let saule_ast::ImportNames::List(items) = names {
                    // ...and in the name list when it is choosing what to pull
                    // in. An `as` alias is a fresh name, so it gets nothing.
                    if items.iter().any(|(n, _)| n == SENTINEL) {
                        self.record(Ctx::ImportName { path: path.clone() });
                    }
                }
            }
        }
    }

    fn params(&mut self, params: &[Param]) {
        for p in params {
            self.ty(Some(&p.ty));
            self.opt_expr(p.default.as_ref());
        }
    }

    /// A sentinel anywhere inside a type annotation means the caret is
    /// writing a type.
    fn ty(&mut self, ty: Option<&Type>) {
        if ty.is_some_and(type_mentions_sentinel) {
            self.record(Ctx::TypeName);
        }
    }

    fn exprs(&mut self, es: &[Spanned<Expr>]) {
        for e in es {
            self.expr(e);
        }
    }

    fn opt_expr(&mut self, e: Option<&Spanned<Expr>>) {
        if let Some(e) = e {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &Spanned<Expr>) {
        match &e.value {
            Expr::Ident(n) if n == SENTINEL => {
                self.record(Ctx::Value { stmt_start: false });
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                if name == SENTINEL {
                    self.record(Ctx::Member((**obj).clone()));
                } else {
                    self.expr(obj);
                }
            }
            Expr::MethodCall { obj, method, args } => {
                if method == SENTINEL {
                    self.record(Ctx::Member((**obj).clone()));
                } else {
                    self.expr(obj);
                    self.args(args);
                }
            }
            Expr::Unary { rhs, .. } => self.expr(rhs),
            Expr::ForceUnwrap(inner) => self.expr(inner),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Index { obj, index } => {
                self.expr(obj);
                self.expr(index);
            }
            Expr::Call { callee, args } => {
                self.expr(callee);
                self.args(args);
            }
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        saule_ast::TableEntry::Positional(v) => self.expr(v),
                        saule_ast::TableEntry::Field { value, .. } => self.expr(value),
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                self.params(params);
                let mark = self.scope.len();
                for p in params {
                    self.bind(&p.name, Some(p.ty.clone()), "parameter");
                }
                match body {
                    LambdaBody::Expr(e) => self.expr(e),
                    LambdaBody::Block(b) => self.block(b),
                }
                self.scope.truncate(mark);
            }
            Expr::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    let mark = self.scope.len();
                    bind_pattern(self, &arm.pattern.value);
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    match &arm.body {
                        MatchBody::Expr(e) => self.expr(e),
                        MatchBody::Block(b) => self.block(b),
                    }
                    self.scope.truncate(mark);
                }
            }
            Expr::Pipe { source, stages } => {
                self.expr(source);
                for st in stages {
                    self.args(&st.args);
                }
            }
            _ => {}
        }
    }

    fn args(&mut self, args: &[CallArg]) {
        for a in args {
            match a {
                CallArg::Positional(e) => self.expr(e),
                CallArg::Named { value, .. } => self.expr(value),
            }
        }
    }
}

/// Patterns bind names inside their arm (`case Event.Key(code) then …`).
pub(crate) fn bind_pattern(w: &mut Walk, p: &saule_ast::Pattern) {
    use saule_ast::Pattern as P;
    match p {
        P::Bind(n) => w.bind(n, None, "pattern binding"),
        P::Variant { fields, .. } => {
            for f in fields {
                bind_pattern(w, &f.value);
            }
        }
        P::Tuple(items) => {
            for i in items {
                bind_pattern(w, &i.value);
            }
        }
        _ => {}
    }
}
