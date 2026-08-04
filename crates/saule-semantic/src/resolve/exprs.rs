//! Resolving expressions, call arguments and patterns, including the
//! argument-ordering and `self.super(...)` rules.

use crate::error::SemanticError;
use crate::to_source_span;
use saule_ast::{CallArg, Expr, LambdaBody, MatchBody, Pattern, Spanned, TableEntry};

use super::*;

impl Resolver {
    pub(crate) fn expr(&mut self, expr: &Spanned<Expr>) {
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
            Expr::ForceUnwrap(inner) => self.expr(inner),
            // Resolve the operand; the target type is resolved separately
            // by the typechecker, which is where unknown type names are
            // reported.
            Expr::Cast { value, .. } => self.expr(value),
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

    pub(crate) fn call_arg(&mut self, a: &CallArg) {
        match a {
            CallArg::Positional(e) | CallArg::Named { value: e, .. } => self.expr(e),
        }
    }

    pub(crate) fn check_arg_ordering(&mut self, args: &[CallArg]) {
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
    pub(crate) fn check_super_receiver(
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
    pub(crate) fn check_super_call(
        &mut self,
        callee: &Spanned<Expr>,
        span: &std::ops::Range<usize>,
    ) {
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

    pub(crate) fn bind_pattern(&mut self, p: &Pattern) {
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
