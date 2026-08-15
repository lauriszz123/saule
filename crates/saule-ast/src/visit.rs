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

/// Call `f` on every expression node in `module`, pre-order.
pub fn visit_exprs<F: FnMut(&Spanned<Expr>)>(module: &Module, f: &mut F) {
    walk_stmts(&module.stmts, f);
}

fn walk_stmts<F: FnMut(&Spanned<Expr>)>(stmts: &[Spanned<Stmt>], f: &mut F) {
    for s in stmts {
        walk_stmt(s, f);
    }
}

fn walk_stmt<F: FnMut(&Spanned<Expr>)>(s: &Spanned<Stmt>, f: &mut F) {
    match &s.value {
        Stmt::Local { value, .. } => walk_opt(value, f),
        Stmt::LocalMulti { values, .. } | Stmt::Return(values) => {
            values.iter().for_each(|e| walk_expr(e, f))
        }
        Stmt::Assign { target, value } | Stmt::CompoundAssign { target, value, .. } => {
            walk_expr(target, f);
            walk_expr(value, f);
        }
        Stmt::AssignMulti { targets, values } => {
            targets.iter().for_each(|e| walk_expr(e, f));
            values.iter().for_each(|e| walk_expr(e, f));
        }
        Stmt::Expr(e) | Stmt::Throw(e) => walk_expr(e, f),
        Stmt::If {
            cond,
            then_block,
            elseifs,
            else_block,
        } => {
            walk_expr(cond, f);
            walk_stmts(then_block, f);
            for (c, b) in elseifs {
                walk_expr(c, f);
                walk_stmts(b, f);
            }
            if let Some(b) = else_block {
                walk_stmts(b, f);
            }
        }
        Stmt::While { cond, body } => {
            walk_expr(cond, f);
            walk_stmts(body, f);
        }
        Stmt::Repeat { body, cond } => {
            walk_stmts(body, f);
            walk_expr(cond, f);
        }
        Stmt::ForNumeric {
            from,
            to,
            step,
            body,
            ..
        } => {
            walk_expr(from, f);
            walk_expr(to, f);
            walk_opt(step, f);
            walk_stmts(body, f);
        }
        Stmt::ForIn { iter, body, .. } => {
            walk_expr(iter, f);
            walk_stmts(body, f);
        }
        Stmt::Try {
            body, catch_body, ..
        } => {
            walk_stmts(body, f);
            walk_stmts(catch_body, f);
        }
        Stmt::Decl(d) => walk_decl(d, f),
        Stmt::Break | Stmt::Continue | Stmt::Error => {}
    }
}

fn walk_decl<F: FnMut(&Spanned<Expr>)>(d: &Spanned<Decl>, f: &mut F) {
    match &d.value {
        Decl::Function { params, body, .. } => {
            walk_params(params, f);
            walk_stmts(body, f);
        }
        Decl::Class { members, .. } => {
            for m in members {
                match &m.value {
                    ClassMember::Field { default, .. } => walk_opt(default, f),
                    ClassMember::Method(me) => walk_method(me, f),
                }
            }
        }
        Decl::Interface { methods, .. } => {
            for sig in methods {
                walk_params(&sig.params, f);
            }
        }
        Decl::Enum {
            variants, methods, ..
        } => {
            for v in variants {
                match &v.value {
                    EnumVariant::Bare(_) => {}
                    EnumVariant::Valued(_, e) => walk_expr(e, f),
                    EnumVariant::Tuple { fields, .. } => walk_params(fields, f),
                }
            }
            for m in methods {
                walk_method(m, f);
            }
        }
        Decl::Variable { value, .. } => walk_opt(value, f),
        Decl::Import { .. } => {}
    }
}

fn walk_method<F: FnMut(&Spanned<Expr>)>(m: &Method, f: &mut F) {
    walk_params(&m.params, f);
    walk_stmts(&m.body, f);
}

fn walk_params<F: FnMut(&Spanned<Expr>)>(params: &[Param], f: &mut F) {
    for p in params {
        walk_opt(&p.default, f);
    }
}

fn walk_opt<F: FnMut(&Spanned<Expr>)>(e: &Option<Spanned<Expr>>, f: &mut F) {
    if let Some(e) = e {
        walk_expr(e, f);
    }
}

fn walk_expr<F: FnMut(&Spanned<Expr>)>(e: &Spanned<Expr>, f: &mut F) {
    f(e);
    match &e.value {
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Nil
        | Expr::Ident(_)
        | Expr::Self_
        | Expr::Error => {}
        Expr::Unary { rhs, .. } => walk_expr(rhs, f),
        Expr::Binary { lhs, rhs, .. } => {
            walk_expr(lhs, f);
            walk_expr(rhs, f);
        }
        Expr::Member { obj, .. } | Expr::SafeMember { obj, .. } => walk_expr(obj, f),
        Expr::Index { obj, index } => {
            walk_expr(obj, f);
            walk_expr(index, f);
        }
        Expr::Call { callee, args } => {
            walk_expr(callee, f);
            walk_args(args, f);
        }
        Expr::ForceUnwrap(inner) => walk_expr(inner, f),
        Expr::Cast { value, .. } => walk_expr(value, f),
        Expr::Table(entries) => {
            for entry in entries {
                match entry {
                    TableEntry::Positional(v) => walk_expr(v, f),
                    TableEntry::Field { key, value } => {
                        walk_expr(key, f);
                        walk_expr(value, f);
                    }
                }
            }
        }
        Expr::Lambda { params, body, .. } => {
            walk_params(params, f);
            match body {
                LambdaBody::Expr(b) => walk_expr(b, f),
                LambdaBody::Block(b) => walk_stmts(b, f),
            }
        }
        Expr::Match { scrutinee, arms } => {
            walk_expr(scrutinee, f);
            for arm in arms {
                walk_opt(&arm.guard, f);
                match &arm.body {
                    MatchBody::Expr(b) => walk_expr(b, f),
                    MatchBody::Block(b) => walk_stmts(b, f),
                }
            }
        }
        Expr::Pipe { source, stages } => {
            walk_expr(source, f);
            for st in stages {
                walk_args(&st.args, f);
            }
        }
    }
}

fn walk_args<F: FnMut(&Spanned<Expr>)>(args: &[CallArg], f: &mut F) {
    for a in args {
        match a {
            CallArg::Positional(e) => walk_expr(e, f),
            CallArg::Named { value, .. } => walk_expr(value, f),
        }
    }
}
