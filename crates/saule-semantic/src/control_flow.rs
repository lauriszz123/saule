//! Control-flow validity check.
//!
//! Walks every statement tracking whether we're currently inside a loop,
//! and reports `break` / `continue` statements that appear outside loop
//! bodies.
//!
//! Lambdas and nested function declarations *reset* the loop context: a
//! `break` inside a lambda that happens to live syntactically inside a
//! `while` does NOT escape the outer loop, so the inner frame is treated
//! as if it were at module scope.
//!
//! `return` placement is *not* restricted — at module top level it's the
//! script's exit value (Lua-style); inside a function body it returns
//! from that function. Either is fine.

use saule_ast::{ClassMember, Decl, Expr, LambdaBody, MatchBody, Spanned, Stmt};

use crate::error::SemanticError;
use crate::to_source_span;

#[derive(Clone, Copy)]
struct Ctx {
    in_loop: bool,
}

impl Ctx {
    const fn top() -> Self {
        Self { in_loop: false }
    }

    const fn enter_loop(self) -> Self {
        Self { in_loop: true }
    }

    /// Crossing a function boundary resets the loop context.
    const fn enter_function() -> Self {
        Self { in_loop: false }
    }
}

pub(crate) fn check_module(module: &saule_ast::Module, errors: &mut Vec<SemanticError>) {
    let ctx = Ctx::top();
    for s in &module.stmts {
        check_stmt(s, ctx, errors);
    }
}

fn check_block(block: &[Spanned<Stmt>], ctx: Ctx, errors: &mut Vec<SemanticError>) {
    for s in block {
        check_stmt(s, ctx, errors);
    }
}

fn check_stmt(stmt: &Spanned<Stmt>, ctx: Ctx, errors: &mut Vec<SemanticError>) {
    match &stmt.value {
        // A recovery hole (see `Stmt::Error`) says only that the text here
        // didn't parse, which the parse diagnostic already reports. Nothing
        // about the enclosing control flow can be concluded from it.
        Stmt::Error => {}
        Stmt::Break => {
            if !ctx.in_loop {
                errors.push(SemanticError::LoopControlOutsideLoop {
                    which: "break",
                    span: to_source_span(stmt.span.clone()),
                });
            }
        }
        Stmt::Continue => {
            if !ctx.in_loop {
                errors.push(SemanticError::LoopControlOutsideLoop {
                    which: "continue",
                    span: to_source_span(stmt.span.clone()),
                });
            }
        }
        Stmt::Return(values) => {
            // Saule/Lua semantics: `return` is valid at every level —
            // inside a function body it returns the function's value, at
            // module top level it ends script load and yields the module's
            // value (used by the REPL and by test harnesses). We therefore
            // only walk the operands; placement isn't restricted.
            for v in values {
                check_expr(v, ctx, errors);
            }
        }

        Stmt::Local { value, .. } => {
            if let Some(v) = value {
                check_expr(v, ctx, errors);
            }
        }
        Stmt::LocalMulti { values, .. } => {
            for v in values {
                check_expr(v, ctx, errors);
            }
        }
        Stmt::Assign { target, value } | Stmt::CompoundAssign { target, value, .. } => {
            check_expr(target, ctx, errors);
            check_expr(value, ctx, errors);
        }
        Stmt::AssignMulti { targets, values } => {
            for t in targets {
                check_expr(t, ctx, errors);
            }
            for v in values {
                check_expr(v, ctx, errors);
            }
        }
        Stmt::Expr(e) | Stmt::Throw(e) => check_expr(e, ctx, errors),

        Stmt::If {
            cond,
            then_block,
            elseifs,
            else_block,
        } => {
            check_expr(cond, ctx, errors);
            check_block(then_block, ctx, errors);
            for (c, b) in elseifs {
                check_expr(c, ctx, errors);
                check_block(b, ctx, errors);
            }
            if let Some(b) = else_block {
                check_block(b, ctx, errors);
            }
        }
        Stmt::While { cond, body } => {
            check_expr(cond, ctx, errors);
            check_block(body, ctx.enter_loop(), errors);
        }
        Stmt::Repeat { body, cond } => {
            check_block(body, ctx.enter_loop(), errors);
            check_expr(cond, ctx, errors);
        }
        Stmt::ForNumeric {
            from,
            to,
            step,
            body,
            ..
        } => {
            check_expr(from, ctx, errors);
            check_expr(to, ctx, errors);
            if let Some(s) = step {
                check_expr(s, ctx, errors);
            }
            check_block(body, ctx.enter_loop(), errors);
        }
        Stmt::ForIn { iter, body, .. } => {
            check_expr(iter, ctx, errors);
            check_block(body, ctx.enter_loop(), errors);
        }
        Stmt::Try {
            body, catch_body, ..
        } => {
            check_block(body, ctx, errors);
            check_block(catch_body, ctx, errors);
        }

        Stmt::Decl(decl) => check_decl(&decl.value, ctx, errors),
    }
}

fn check_decl(decl: &Decl, ctx: Ctx, errors: &mut Vec<SemanticError>) {
    match decl {
        Decl::Function { body, params, .. } => {
            for p in params {
                if let Some(d) = &p.default {
                    check_expr(d, ctx, errors);
                }
            }
            check_block(body, Ctx::enter_function(), errors);
        }
        Decl::Class { members, .. } => {
            for m in members {
                match &m.value {
                    ClassMember::Method(meth) => {
                        for p in &meth.params {
                            if let Some(d) = &p.default {
                                check_expr(d, ctx, errors);
                            }
                        }
                        check_block(&meth.body, Ctx::enter_function(), errors);
                    }
                    ClassMember::Field {
                        default: Some(d), ..
                    } => {
                        check_expr(d, ctx, errors);
                    }
                    ClassMember::Field { .. } => {}
                }
            }
        }
        Decl::Enum { methods, .. } => {
            for meth in methods {
                for p in &meth.params {
                    if let Some(d) = &p.default {
                        check_expr(d, ctx, errors);
                    }
                }
                check_block(&meth.body, Ctx::enter_function(), errors);
            }
        }
        // The initializer runs at module scope, so `break` / `continue` in
        // it are as invalid as anywhere else outside a loop — `ctx` is
        // passed through unchanged to say so.
        Decl::Variable { value, .. } => {
            if let Some(v) = value {
                check_expr(v, ctx, errors);
            }
        }
        // Interface / Import declarations have no executable body.
        Decl::Interface { .. } | Decl::Import { .. } => {}
    }
}

fn check_expr(expr: &Spanned<Expr>, ctx: Ctx, errors: &mut Vec<SemanticError>) {
    match &expr.value {
        Expr::Error => {}
        Expr::Unary { rhs, .. } => check_expr(rhs, ctx, errors),
        Expr::Binary { lhs, rhs, .. } => {
            check_expr(lhs, ctx, errors);
            check_expr(rhs, ctx, errors);
        }
        Expr::Member { obj, .. } | Expr::SafeMember { obj, .. } => check_expr(obj, ctx, errors),
        // A cast is transparent to control flow — it wraps a value, it
        // doesn't branch or return.
        Expr::Cast { value, .. } => check_expr(value, ctx, errors),
        Expr::Index { obj, index } => {
            check_expr(obj, ctx, errors);
            check_expr(index, ctx, errors);
        }
        Expr::Call { callee, args } => {
            check_expr(callee, ctx, errors);
            for a in args {
                check_call_arg(a, ctx, errors);
            }
        }
        Expr::ForceUnwrap(inner) => check_expr(inner, ctx, errors),
        Expr::Table(entries) => {
            for e in entries {
                match e {
                    saule_ast::TableEntry::Positional(v) => check_expr(v, ctx, errors),
                    saule_ast::TableEntry::Field { key, value } => {
                        check_expr(key, ctx, errors);
                        check_expr(value, ctx, errors);
                    }
                }
            }
        }
        Expr::Lambda { body, params, .. } => {
            for p in params {
                if let Some(d) = &p.default {
                    check_expr(d, ctx, errors);
                }
            }
            let inner = Ctx::enter_function();
            match body {
                LambdaBody::Expr(e) => check_expr(e, inner, errors),
                LambdaBody::Block(b) => check_block(b, inner, errors),
            }
        }
        Expr::Match { scrutinee, arms } => {
            check_expr(scrutinee, ctx, errors);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    check_expr(g, ctx, errors);
                }
                match &arm.body {
                    MatchBody::Expr(e) => check_expr(e, ctx, errors),
                    MatchBody::Block(b) => check_block(b, ctx, errors),
                }
            }
        }
        Expr::Pipe { source, stages } => {
            check_expr(source, ctx, errors);
            for stage in stages {
                for a in &stage.args {
                    check_call_arg(a, ctx, errors);
                }
            }
        }
        // Leaves: no nested expressions.
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Nil
        | Expr::Ident(_)
        | Expr::Self_ => {}
    }
}

fn check_call_arg(arg: &saule_ast::CallArg, ctx: Ctx, errors: &mut Vec<SemanticError>) {
    match arg {
        saule_ast::CallArg::Positional(e) | saule_ast::CallArg::Named { value: e, .. } => {
            check_expr(e, ctx, errors)
        }
    }
}
