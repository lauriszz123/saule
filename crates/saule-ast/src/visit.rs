//! A read-only walk over every expression in a module.
//!
//! [`assign_ids`](crate::assign_ids) already knows the shape of the tree, but
//! it needs `&mut` and it numbers *all* nodes. Passes that want to survey
//! expressions specifically — "how many of these did inference reach?",
//! "which arithmetic operands have a known type?" — need the shared-reference
//! version, so it lives here rather than being re-derived per crate.
//!
//! The traversal order matches `assign_ids`: pre-order, parent before
//! children, in source order.

use crate::{
    CallArg, ClassMember, Decl, EnumVariant, Expr, LambdaBody, MatchBody, Method, Module, Param,
    Spanned, Stmt, TableEntry,
};

/// A read-only visitor over a module.
///
/// The traversal is defined once, here, and callers pick which events they
/// care about — which is why this is a trait rather than a second walker
/// per question. Every method has a default, so implementing one costs
/// nothing for the rest.
pub trait Visitor {
    /// Every expression node, pre-order.
    fn expr(&mut self, _e: &Spanned<Expr>) {}
    /// The *target* of an assignment, before it is walked as an expression.
    ///
    /// Position is the whole point: `x.f` reads and `x.f = 1` writes, and
    /// the two are indistinguishable once flattened into [`Self::expr`].
    fn assign_target(&mut self, _e: &Spanned<Expr>) {}
}

/// Adapter so the common "just the expressions" case stays a closure.
struct ExprsOnly<F>(F);

impl<F: FnMut(&Spanned<Expr>)> Visitor for ExprsOnly<F> {
    fn expr(&mut self, e: &Spanned<Expr>) {
        (self.0)(e)
    }
}

/// Call `f` on every expression node in `module`, pre-order.
pub fn visit_exprs<F: FnMut(&Spanned<Expr>)>(module: &Module, f: &mut F) {
    let mut v = ExprsOnly(f);
    walk_stmts(&module.stmts, &mut v);
}

/// Walk `module` with a full [`Visitor`].
pub fn visit(module: &Module, v: &mut impl Visitor) {
    walk_stmts(&module.stmts, v);
}

/// Walk a statement list with a full [`Visitor`].
///
/// The same traversal [`visit`] performs, exposed for the passes that hold a
/// body rather than a whole module — "which top-level names does this
/// function reach?" is asked of one `fn` at a time.
pub fn visit_stmts(stmts: &[Spanned<Stmt>], v: &mut impl Visitor) {
    walk_stmts(stmts, v);
}

fn walk_stmts<V: Visitor>(stmts: &[Spanned<Stmt>], v: &mut V) {
    for s in stmts {
        walk_stmt(s, v);
    }
}

fn walk_stmt<V: Visitor>(s: &Spanned<Stmt>, v: &mut V) {
    match &s.value {
        Stmt::Local { value, .. } => walk_opt(value, v),
        Stmt::LocalMulti { values, .. } | Stmt::Return(values) => {
            values.iter().for_each(|e| walk_expr(e, v))
        }
        Stmt::Assign { target, value } | Stmt::CompoundAssign { target, value, .. } => {
            v.assign_target(target);
            walk_expr(target, v);
            walk_expr(value, v);
        }
        Stmt::AssignMulti { targets, values } => {
            for t in targets {
                v.assign_target(t);
                walk_expr(t, v);
            }
            values.iter().for_each(|e| walk_expr(e, v));
        }
        Stmt::Expr(e) | Stmt::Throw(e) => walk_expr(e, v),
        Stmt::If {
            cond,
            then_block,
            elseifs,
            else_block,
        } => {
            walk_expr(cond, v);
            walk_stmts(then_block, v);
            for (c, b) in elseifs {
                walk_expr(c, v);
                walk_stmts(b, v);
            }
            if let Some(b) = else_block {
                walk_stmts(b, v);
            }
        }
        Stmt::While { cond, body } => {
            walk_expr(cond, v);
            walk_stmts(body, v);
        }
        Stmt::Repeat { body, cond } => {
            walk_stmts(body, v);
            walk_expr(cond, v);
        }
        Stmt::ForNumeric {
            from,
            to,
            step,
            body,
            ..
        } => {
            walk_expr(from, v);
            walk_expr(to, v);
            walk_opt(step, v);
            walk_stmts(body, v);
        }
        Stmt::ForIn { iter, body, .. } => {
            walk_expr(iter, v);
            walk_stmts(body, v);
        }
        Stmt::Try {
            body, catch_body, ..
        } => {
            walk_stmts(body, v);
            walk_stmts(catch_body, v);
        }
        Stmt::Decl(d) => walk_decl(d, v),
        Stmt::Break | Stmt::Continue | Stmt::Error => {}
    }
}

fn walk_decl<V: Visitor>(d: &Spanned<Decl>, v: &mut V) {
    match &d.value {
        Decl::Function { params, body, .. } => {
            walk_params(params, v);
            walk_stmts(body, v);
        }
        Decl::Class { members, .. } => {
            for m in members {
                match &m.value {
                    ClassMember::Field { default, .. } => walk_opt(default, v),
                    ClassMember::Method(me) => walk_method(me, v),
                }
            }
        }
        Decl::Interface { methods, .. } => {
            for sig in methods {
                walk_params(&sig.params, v);
            }
        }
        Decl::Enum {
            variants, methods, ..
        } => {
            for variant in variants {
                match &variant.value {
                    EnumVariant::Bare(_) => {}
                    EnumVariant::Valued(_, e) => walk_expr(e, v),
                    EnumVariant::Tuple { fields, .. } => walk_params(fields, v),
                }
            }
            for m in methods {
                walk_method(m, v);
            }
        }
        Decl::Variable { value, .. } => walk_opt(value, v),
        Decl::Import { .. } => {}
    }
}

fn walk_method<V: Visitor>(m: &Method, v: &mut V) {
    walk_params(&m.params, v);
    walk_stmts(&m.body, v);
}

fn walk_params<V: Visitor>(params: &[Param], v: &mut V) {
    for p in params {
        walk_opt(&p.default, v);
    }
}

fn walk_opt<V: Visitor>(e: &Option<Spanned<Expr>>, v: &mut V) {
    if let Some(e) = e {
        walk_expr(e, v);
    }
}

fn walk_expr<V: Visitor>(e: &Spanned<Expr>, v: &mut V) {
    v.expr(e);
    match &e.value {
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Nil
        | Expr::Ident(_)
        | Expr::Self_
        | Expr::Error => {}
        Expr::Unary { rhs, .. } => walk_expr(rhs, v),
        Expr::Binary { lhs, rhs, .. } => {
            walk_expr(lhs, v);
            walk_expr(rhs, v);
        }
        Expr::Member { obj, .. } | Expr::SafeMember { obj, .. } => walk_expr(obj, v),
        Expr::Index { obj, index } => {
            walk_expr(obj, v);
            walk_expr(index, v);
        }
        Expr::Call { callee, args, .. } => {
            walk_expr(callee, v);
            walk_args(args, v);
        }
        Expr::ForceUnwrap(inner) => walk_expr(inner, v),
        Expr::Cast { value, .. } => walk_expr(value, v),
        Expr::Table(entries) => {
            for entry in entries {
                match entry {
                    TableEntry::Positional(item) => walk_expr(item, v),
                    TableEntry::Field { key, value } => {
                        walk_expr(key, v);
                        walk_expr(value, v);
                    }
                }
            }
        }
        Expr::Lambda { params, body, .. } => {
            walk_params(params, v);
            match body {
                LambdaBody::Expr(b) => walk_expr(b, v),
                LambdaBody::Block(b) => walk_stmts(b, v),
            }
        }
        Expr::Match { scrutinee, arms } => {
            walk_expr(scrutinee, v);
            for arm in arms {
                walk_opt(&arm.guard, v);
                match &arm.body {
                    MatchBody::Expr(b) => walk_expr(b, v),
                    MatchBody::Block(b) => walk_stmts(b, v),
                }
            }
        }
        Expr::Pipe { source, stages } => {
            walk_expr(source, v);
            for st in stages {
                walk_args(&st.args, v);
            }
        }
    }
}

fn walk_args<V: Visitor>(args: &[CallArg], v: &mut V) {
    for a in args {
        match a {
            CallArg::Positional(e) => walk_expr(e, v),
            CallArg::Named { value, .. } => walk_expr(value, v),
        }
    }
}
