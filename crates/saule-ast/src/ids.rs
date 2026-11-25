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

use std::ops::Range;
use std::sync::Arc;

use crate::{
    CallArg, ClassMember, Decl, EnumVariant, Expr, LambdaBody, MatchBody, Method, MethodSig,
    Module, NodeId, Param, Pattern, Spanned, Stmt, TableEntry, Type, TypeBound, TypeRef,
};

/// Number every node in `module`, pre-order, starting at 0.
///
/// Returns the number of ids assigned, which is also the exclusive upper
/// bound on any id in the tree — so a caller can size a `Vec`-backed side
/// table instead of a `HashMap` if it wants to.
///
/// Idempotent in effect: running it twice produces the same numbering.
pub fn assign_ids(module: &mut Module) -> usize {
    let mut w = Walk {
        next: Some(0),
        hook: |_: &mut Spanned<Expr>| {},
        types: |_: &mut Type, _: TypeSite<'_>| {},
        scope: Vec::new(),
    };
    w.stmts(&mut module.stmts);
    w.next.unwrap_or(0) as usize
}

/// Call `f` on every expression node in `module`, pre-order, with mutable
/// access — the `&mut` counterpart to
/// [`visit_exprs`](crate::visit_exprs).
///
/// Numbering is left alone, so this is safe to run on an already-numbered
/// tree: side tables keyed by [`NodeId`] stay valid across it.
///
/// It shares [`assign_ids`]' traversal rather than defining a second one.
/// The shape of the tree is written down once, so a new [`Expr`] variant
/// cannot be reached by one walk and missed by the other.
pub fn visit_exprs_mut<F: FnMut(&mut Spanned<Expr>)>(module: &mut Module, f: F) {
    let mut w = Walk {
        next: None,
        hook: f,
        types: |_: &mut Type, _: TypeSite<'_>| {},
        scope: Vec::new(),
    };
    w.stmts(&mut module.stmts);
}

/// Call `f` on every type written in `module`, with mutable access: the
/// annotation of a local, a parameter, a field or a return, the target of a
/// cast, the type of a `catch`, the arguments in `extends Base<T>` and
/// `f<T>(…)`, a type parameter's bound, and the right-hand side of a
/// `type` alias.
///
/// `f` is handed each type *whole* — `table<string, Handler>?`, not its
/// parts — and does its own descent, since what it wants of the parts
/// depends on what it is doing. With it comes a [`TypeSite`]: where the type
/// stands, and which type parameters are in scope there.
///
/// Numbering is left alone, as with [`visit_exprs_mut`], and for the same
/// reason this shares that traversal: a new place a type can be written is
/// then one line here, not a second walk to keep in step.
pub fn visit_types_mut<F: FnMut(&mut Type, TypeSite<'_>)>(module: &mut Module, f: F) {
    let mut w = Walk {
        next: None,
        hook: |_: &mut Spanned<Expr>| {},
        types: f,
        scope: Vec::new(),
    };
    w.stmts(&mut module.stmts);
}

/// Where a type handed to [`visit_types_mut`]'s callback was written.
#[derive(Clone, Copy)]
pub struct TypeSite<'a> {
    /// The narrowest span the tree records around it: the annotation itself
    /// for a local or a module variable, otherwise the parameter, member,
    /// clause or expression it is part of. A [`Type`] carries no span of its
    /// own.
    pub span: &'a Range<usize>,
    /// The type parameters in scope, outermost first: those of the enclosing
    /// class, then of the method, and so on. A name among them means the
    /// parameter, whatever else goes by that name.
    pub type_params: &'a [String],
}

/// The one mutable pre-order traversal.
///
/// `next` is `Some` only when numbering; `hook` runs on every expression,
/// and `types` on every type written (see [`visit_types_mut`]).
/// Both jobs walk the identical shape, and the pair of `Option` / closure
/// is what keeps that shape from being written twice.
struct Walk<F, G> {
    next: Option<u32>,
    hook: F,
    types: G,
    /// The type parameters in scope where the walk stands — see
    /// [`TypeSite::type_params`].
    scope: Vec<String>,
}

impl<F: FnMut(&mut Spanned<Expr>), G: FnMut(&mut Type, TypeSite<'_>)> Walk<F, G> {
    fn ty(&mut self, ty: &mut Type, span: &Range<usize>) {
        let site = TypeSite {
            span,
            type_params: &self.scope,
        };
        (self.types)(ty, site);
    }

    fn opt_ty(&mut self, ty: &mut Option<Type>, span: &Range<usize>) {
        if let Some(ty) = ty {
            self.ty(ty, span);
        }
    }

    fn tys(&mut self, types: &mut [Type], span: &Range<usize>) {
        for ty in types {
            self.ty(ty, span);
        }
    }

    /// The arguments of `extends Base<A>` / `implements Repo<A>`. The name
    /// itself is a class or an interface, never an alias.
    fn type_refs(&mut self, refs: &mut [TypeRef], span: &Range<usize>) {
        for r in refs {
            self.tys(&mut r.args, span);
        }
    }

    fn bounds(&mut self, bounds: &mut [TypeBound]) {
        for b in bounds {
            let span = b.span.clone();
            self.tys(&mut b.bound.args, &span);
        }
    }

    /// Run `body` with `params` in scope as type parameters.
    fn with_type_params(&mut self, params: &[String], body: impl FnOnce(&mut Self)) {
        let outer = self.scope.len();
        self.scope.extend(params.iter().cloned());
        body(self);
        self.scope.truncate(outer);
    }

    fn method_sig(&mut self, sig: &mut MethodSig) {
        let span = sig.span.clone();
        let MethodSig {
            type_params,
            type_bounds,
            params,
            return_ty,
            ..
        } = sig;
        self.with_type_params(type_params, |w| {
            w.bounds(type_bounds);
            w.params(params);
            w.opt_ty(return_ty, &span);
        });
    }

    /// Stamp a node and return its id. Pre-order: the parent is numbered
    /// before its children. A no-op when this walk is not numbering.
    fn stamp<T>(&mut self, node: &mut Spanned<T>) {
        if let Some(next) = &mut self.next {
            node.id = NodeId(*next);
            *next += 1;
        }
    }

    fn stmts(&mut self, stmts: &mut [Spanned<Stmt>]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &mut Spanned<Stmt>) {
        self.stamp(s);
        let span = s.span.clone();
        match &mut s.value {
            Stmt::Local {
                ty, ty_span, value, ..
            } => {
                self.opt_ty(ty, ty_span.as_ref().unwrap_or(&span));
                self.opt_expr(value);
            }
            Stmt::LocalMulti { names, values } => {
                for (_, name_span, ty) in names {
                    self.opt_ty(ty, name_span);
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
                var_ty,
                from,
                to,
                step,
                body,
                ..
            } => {
                self.opt_ty(var_ty, &span);
                self.expr(from);
                self.expr(to);
                self.opt_expr(step);
                self.stmts(body);
            }
            Stmt::ForIn { vars, iter, body } => {
                for (_, ty) in vars {
                    self.opt_ty(ty, &span);
                }
                self.expr(iter);
                self.stmts(body);
            }
            Stmt::Return(es) => self.exprs(es),
            Stmt::Try {
                body,
                catches,
                finally,
            } => {
                self.stmts(body);
                for c in catches {
                    let clause = c.span.clone();
                    self.ty(&mut c.ty, &clause);
                    self.stmts(&mut c.body);
                }
                if let Some(f) = finally {
                    self.stmts(f);
                }
            }
            Stmt::Decl(d) => self.decl(d),
            Stmt::Break | Stmt::Continue | Stmt::Error => {}
        }
    }

    fn decl(&mut self, d: &mut Spanned<Decl>) {
        self.stamp(d);
        let span = d.span.clone();
        match &mut d.value {
            Decl::Function {
                type_params,
                type_bounds,
                params,
                return_ty,
                body,
                ..
            } => self.with_type_params(type_params, |w| {
                w.bounds(type_bounds);
                w.params(params);
                w.opt_ty(return_ty, &span);
                w.stmts(body);
            }),
            Decl::Class {
                type_params,
                type_bounds,
                extends,
                implements,
                members,
                ..
            } => self.with_type_params(type_params, |w| {
                w.bounds(type_bounds);
                if let Some(parent) = extends {
                    w.tys(&mut parent.args, &span);
                }
                w.type_refs(implements, &span);
                for m in members {
                    w.stamp(m);
                    let member = m.span.clone();
                    match &mut m.value {
                        ClassMember::Field { ty, default, .. } => {
                            w.ty(ty, &member);
                            w.opt_expr(default);
                        }
                        ClassMember::Method(me) => w.method(me),
                    }
                }
            }),
            Decl::Interface {
                type_params,
                type_bounds,
                extends,
                methods,
                ..
            } => self.with_type_params(type_params, |w| {
                w.bounds(type_bounds);
                w.type_refs(extends, &span);
                for sig in methods {
                    w.method_sig(sig);
                }
            }),
            Decl::Enum {
                type_params,
                type_bounds,
                variants,
                methods,
                ..
            } => self.with_type_params(type_params, |w| {
                w.bounds(type_bounds);
                for v in variants {
                    w.stamp(v);
                    match &mut v.value {
                        EnumVariant::Bare(_) => {}
                        EnumVariant::Valued(_, e) => w.expr(e),
                        EnumVariant::Tuple { fields, .. } => w.params(fields),
                    }
                }
                for m in methods {
                    w.method(m);
                }
            }),
            Decl::Variable {
                ty, ty_span, value, ..
            } => {
                self.opt_ty(ty, ty_span.as_ref().unwrap_or(&span));
                self.opt_expr(value);
            }
            Decl::TypeAlias {
                type_params,
                ty,
                ty_span,
                ..
            } => self.with_type_params(type_params, |w| w.ty(ty, ty_span)),
            Decl::Import { .. } => {}
        }
    }

    fn method(&mut self, m: &mut Method) {
        let span = m.span.clone();
        let Method {
            type_params,
            type_bounds,
            params,
            return_ty,
            body,
            ..
        } = m;
        self.with_type_params(type_params, |w| {
            w.bounds(type_bounds);
            w.params(params);
            w.opt_ty(return_ty, &span);
            w.stmts(body);
        });
    }

    /// A `Param` is not itself a `Spanned`, but its default **is** an
    /// expression that gets evaluated, so it needs an id like any other.
    fn params(&mut self, params: &mut [Param]) {
        for p in params {
            let span = p.span.clone();
            self.ty(&mut p.ty, &span);
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
        (self.hook)(e);
        let span = e.span.clone();
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
            Expr::Index { obj, index } | Expr::SafeIndex { obj, index } => {
                self.expr(obj);
                self.expr(index);
            }
            Expr::Call {
                callee,
                args,
                type_args,
            } => {
                self.expr(callee);
                self.args(args);
                if let Some(written) = type_args {
                    let at = written.span.clone();
                    self.tys(&mut written.types, &at);
                }
            }
            Expr::ForceUnwrap(inner) => self.expr(inner),
            Expr::Cast { value, ty, .. } | Expr::Is { value, ty, .. } => {
                self.expr(value);
                self.ty(ty, &span);
            }
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
            Expr::Lambda {
                params,
                return_ty,
                body,
            } => {
                self.params(params);
                self.opt_ty(return_ty, &span);
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
                    if let Some(written) = &mut stage.type_args {
                        let at = written.span.clone();
                        self.tys(&mut written.types, &at);
                    }
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
        assert!(
            !b[0].id.is_none(),
            "lambda body statement was left unnumbered"
        );
        let Stmt::Return(rs) = &b[0].value else {
            unreachable!()
        };
        assert!(!rs[0].id.is_none());
    }
}
