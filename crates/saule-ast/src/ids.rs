//! Node numbering: [`assign_ids`].
//!
//! The parser does **not** assign ids. `Spanned::new` leaves every node at
//! [`NodeId::NONE`] and this pass numbers them afterwards, in one
//! deterministic pre-order walk. That split is what keeps all 73
//! `Spanned::new` call sites in the parser untouched, and it means the
//! numbering rule lives in one readable function instead of being smeared
//! across the parser.
//!
//! **Determinism is the contract.** Two runs over the same source must
//! produce the same ids, because side tables built by one pass are read by
//! another (`saule-typeck`'s type table, `saule-semantic`'s binding table),
//! and a bytecode cache would key on them across processes.

use std::sync::Arc;

use crate::{
    CallArg, ClassMember, Decl, EnumVariant, Expr, LambdaBody, MatchBody, Method, Module, NodeId,
    Param, Pattern, Spanned, Stmt, TableEntry,
};

/// Number every node in `module`, pre-order, starting at 0.
///
/// Returns the number of ids assigned, which is also the exclusive upper
/// bound on any id in the tree — so a caller can size a `Vec`-backed side
/// table instead of a `HashMap` if it wants to.
///
/// Idempotent in effect: running it twice produces the same numbering.
pub fn assign_ids(module: &mut Module) -> usize {
    let mut w = Walk { next: 0 };
    w.stmts(&mut module.stmts);
    w.next as usize
}

struct Walk {
    next: u32,
}

impl Walk {
    /// Stamp a node and return its id. Pre-order: the parent is numbered
    /// before its children.
    fn stamp<T>(&mut self, node: &mut Spanned<T>) {
        node.id = NodeId(self.next);
        self.next += 1;
    }

    fn stmts(&mut self, stmts: &mut [Spanned<Stmt>]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &mut Spanned<Stmt>) {
        self.stamp(s);
        match &mut s.value {
            Stmt::Local { value, .. } => self.opt_expr(value),
            Stmt::LocalMulti { values, .. } => self.exprs(values),
            Stmt::Assign { target, value } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::AssignMulti { targets, values } => {
                self.exprs(targets);
                self.exprs(values);
            }
            Stmt::CompoundAssign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::Expr(e) | Stmt::Throw(e) => self.expr(e),
            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
                self.expr(cond);
                self.stmts(then_block);
                for (c, b) in elseifs {
                    self.expr(c);
                    self.stmts(b);
                }
                if let Some(b) = else_block {
                    self.stmts(b);
                }
            }
            Stmt::While { cond, body } => {
                self.expr(cond);
                self.stmts(body);
            }
            Stmt::Repeat { body, cond } => {
                // Source order: the body is written before the `until`.
                self.stmts(body);
                self.expr(cond);
            }
            Stmt::ForNumeric {
                from,
                to,
                step,
                body,
                ..
            } => {
                self.expr(from);
                self.expr(to);
                self.opt_expr(step);
                self.stmts(body);
            }
            Stmt::ForIn { iter, body, .. } => {
                self.expr(iter);
                self.stmts(body);
            }
            Stmt::Return(es) => self.exprs(es),
            Stmt::Try {
                body, catch_body, ..
            } => {
                self.stmts(body);
                self.stmts(catch_body);
            }
            Stmt::Decl(d) => self.decl(d),
            Stmt::Break | Stmt::Continue | Stmt::Error => {}
        }
    }

    fn decl(&mut self, d: &mut Spanned<Decl>) {
        self.stamp(d);
        match &mut d.value {
            Decl::Function { params, body, .. } => {
                self.params(params);
                self.stmts(body);
            }
            Decl::Class { members, .. } => {
                for m in members {
                    self.stamp(m);
                    match &mut m.value {
                        ClassMember::Field { default, .. } => self.opt_expr(default),
                        ClassMember::Method(me) => self.method(me),
                    }
                }
            }
            Decl::Interface { methods, .. } => {
                for sig in methods {
                    self.params(&mut sig.params);
                }
            }
            Decl::Enum {
                variants, methods, ..
            } => {
                for v in variants {
                    self.stamp(v);
                    match &mut v.value {
                        EnumVariant::Bare(_) => {}
                        EnumVariant::Valued(_, e) => self.expr(e),
                        EnumVariant::Tuple { fields, .. } => self.params(fields),
                    }
                }
                for m in methods {
                    self.method(m);
                }
            }
            Decl::Variable { value, .. } => self.opt_expr(value),
            Decl::Import { .. } => {}
        }
    }

    fn method(&mut self, m: &mut Method) {
        self.params(&mut m.params);
        self.stmts(&mut m.body);
    }

    /// A `Param` is not itself a `Spanned`, but its default **is** an
    /// expression that gets evaluated, so it needs an id like any other.
    fn params(&mut self, params: &mut [Param]) {
        for p in params {
            self.opt_expr(&mut p.default);
        }
    }

    fn exprs(&mut self, es: &mut [Spanned<Expr>]) {
        for e in es {
            self.expr(e);
        }
    }

    fn opt_expr(&mut self, e: &mut Option<Spanned<Expr>>) {
        if let Some(e) = e {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &mut Spanned<Expr>) {
        self.stamp(e);
        match &mut e.value {
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::Str(_)
            | Expr::Nil
            | Expr::Ident(_)
            | Expr::Self_
            | Expr::Error => {}
            Expr::Unary { rhs, .. } => self.expr(rhs),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Member { obj, .. } | Expr::SafeMember { obj, .. } => self.expr(obj),
            Expr::Index { obj, index } => {
                self.expr(obj);
                self.expr(index);
            }
            Expr::Call { callee, args } => {
                self.expr(callee);
                self.args(args);
            }
            Expr::ForceUnwrap(inner) => self.expr(inner),
            Expr::Cast { value, .. } => self.expr(value),
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        TableEntry::Positional(v) => self.expr(v),
                        TableEntry::Field { key, value } => {
                            self.expr(key);
                            self.expr(value);
                        }
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                self.params(params);
                self.lambda_body(body);
            }
            Expr::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.pattern(&mut arm.pattern);
                    self.opt_expr(&mut arm.guard);
                    match &mut arm.body {
                        MatchBody::Expr(e) => self.expr(e),
                        MatchBody::Block(b) => self.stmts(b),
                    }
                }
            }
            Expr::Pipe { source, stages } => {
                self.expr(source);
                for stage in stages {
                    self.args(&mut stage.args);
                }
            }
        }
    }

    /// A lambda body is behind an `Arc` so that evaluating a lambda inside a
    /// loop is a refcount bump rather than a deep copy. `make_mut` rather
    /// than `get_mut`: right after parsing the `Arc` is unique so this is
    /// free, and if it ever is not, cloning is far better than silently
    /// leaving a whole body unnumbered.
    fn lambda_body(&mut self, body: &mut LambdaBody) {
        match body {
            LambdaBody::Expr(e) => self.expr(Arc::make_mut(e)),
            LambdaBody::Block(b) => self.stmts(Arc::make_mut(b)),
        }
    }

    fn args(&mut self, args: &mut [CallArg]) {
        for a in args {
            match a {
                CallArg::Positional(e) => self.expr(e),
                CallArg::Named { value, .. } => self.expr(value),
            }
        }
    }

    fn pattern(&mut self, p: &mut Spanned<Pattern>) {
        self.stamp(p);
        match &mut p.value {
            Pattern::Variant { fields, .. } | Pattern::Tuple(fields) => {
                for f in fields {
                    self.pattern(f);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_of(stmts: Vec<Spanned<Stmt>>) -> Module {
        Module { stmts }
    }

    fn int(n: i64) -> Spanned<Expr> {
        Spanned::new(Expr::Int(n), 0..1)
    }

    #[test]
    fn numbers_pre_order_from_zero() {
        // `local x = 1 + 2` — stmt, then the binary, then each operand.
        let mut m = module_of(vec![Spanned::new(
            Stmt::Local {
                name: "x".into(),
                name_span: 0..1,
                ty: None,
                ty_span: None,
                value: Some(Spanned::new(
                    Expr::Binary {
                        op: crate::BinOp::Add,
                        lhs: Box::new(int(1)),
                        rhs: Box::new(int(2)),
                    },
                    0..5,
                )),
            },
            0..9,
        )]);

        assert_eq!(assign_ids(&mut m), 4);
        assert_eq!(m.stmts[0].id, NodeId(0));
        let Stmt::Local { value: Some(v), .. } = &m.stmts[0].value else {
            unreachable!()
        };
        assert_eq!(v.id, NodeId(1));
        let Expr::Binary { lhs, rhs, .. } = &v.value else {
            unreachable!()
        };
        assert_eq!(lhs.id, NodeId(2));
        assert_eq!(rhs.id, NodeId(3));
    }

    #[test]
    fn is_deterministic() {
        let build = || module_of(vec![Spanned::new(Stmt::Expr(int(7)), 0..1)]);
        let (mut a, mut b) = (build(), build());
        assert_eq!(assign_ids(&mut a), assign_ids(&mut b));
        assert_eq!(a.stmts[0].id, b.stmts[0].id);
    }

    #[test]
    fn ids_are_invisible_to_equality() {
        // The property the parser's tests depend on.
        let mut numbered = module_of(vec![Spanned::new(Stmt::Expr(int(7)), 0..1)]);
        let bare = module_of(vec![Spanned::new(Stmt::Expr(int(7)), 0..1)]);
        assign_ids(&mut numbered);
        assert_ne!(numbered.stmts[0].id, bare.stmts[0].id);
        assert_eq!(numbered, bare);
    }

    #[test]
    fn reaches_inside_a_lambda_body() {
        let lambda = Spanned::new(
            Expr::Lambda {
                params: vec![],
                return_ty: None,
                body: LambdaBody::Block(Arc::from(vec![Spanned::new(
                    Stmt::Return(vec![int(1)]),
                    0..1,
                )])),
            },
            0..9,
        );
        let mut m = module_of(vec![Spanned::new(Stmt::Expr(lambda), 0..9)]);
        assign_ids(&mut m);

        let Stmt::Expr(l) = &m.stmts[0].value else {
            unreachable!()
        };
        let Expr::Lambda {
            body: LambdaBody::Block(b),
            ..
        } = &l.value
        else {
            unreachable!()
        };
        assert!(!b[0].id.is_none(), "lambda body statement was left unnumbered");
        let Stmt::Return(rs) = &b[0].value else {
            unreachable!()
        };
        assert!(!rs[0].id.is_none());
    }
}
