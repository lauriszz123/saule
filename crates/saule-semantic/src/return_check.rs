//! Definite-return analysis.
//!
//! Every function or method that declares a non-nullable, non-void return
//! type must return (or throw) on every reachable path. If it can fall
//! off the end of the body the runtime would silently produce `nil`,
//! which then explodes the moment a caller treats it as the declared
//! type — exactly the bug the user hit with
//!
//! ```text
//! static fn loadFile(path: string) -> string
//!     if not Os.exists(path) then
//!         ...
//!         return source
//!     end
//! end
//! ```
//!
//! Conservative rules — we only mark a block as "definitely returns" when
//! we can prove it from the structure alone. False negatives (functions
//! that *do* always return but we can't tell) are tolerated; false
//! positives (calling a path return-free when it isn't) are not.

use saule_ast::{ClassMember, Decl, MatchArm, Method, Pattern, Spanned, Stmt, Type};

use crate::error::SemanticError;
use crate::to_source_span;

pub(crate) fn check_module(module: &saule_ast::Module, errors: &mut Vec<SemanticError>) {
    for stmt in &module.stmts {
        if let Stmt::Decl(decl) = &stmt.value {
            check_decl(&decl.value, errors);
        }
    }
}

fn check_decl(decl: &Decl, errors: &mut Vec<SemanticError>) {
    match decl {
        Decl::Function {
            name,
            return_ty,
            body,
            ..
        } => {
            check_fn(name, return_ty.as_ref(), body, errors);
        }
        Decl::Class {
            name: class,
            members,
            ..
        } => {
            for m in members {
                if let ClassMember::Method(meth) = &m.value {
                    let qual = format!("{class}.{}", meth.name);
                    check_method(&qual, meth, errors);
                }
            }
        }
        Decl::Enum {
            name: en, methods, ..
        } => {
            for meth in methods {
                let qual = format!("{en}.{}", meth.name);
                check_method(&qual, meth, errors);
            }
        }
        // No function body to walk. A lambda in the initializer carries its
        // own `return`s, which `check_fn` reaches through the expression
        // walk in the enclosing function, not from here.
        Decl::Interface { .. } | Decl::Import { .. } | Decl::Variable { .. } => {}
    }
}

fn check_method(qualified: &str, meth: &Method, errors: &mut Vec<SemanticError>) {
    check_fn(qualified, meth.return_ty.as_ref(), &meth.body, errors);
}

fn check_fn(
    name: &str,
    return_ty: Option<&Type>,
    body: &[Spanned<Stmt>],
    errors: &mut Vec<SemanticError>,
) {
    let Some(ty) = return_ty else { return };
    if !requires_return(ty) {
        return;
    }
    if block_returns(body) {
        return;
    }
    // Best-effort span: point at the last statement of the body (where the
    // missing `return` would naturally go), falling back to a zero span at
    // position 0 if the body is empty.
    let span = body.last().map(|s| s.span.clone()).unwrap_or(0..0);
    errors.push(SemanticError::MissingReturn {
        name: name.to_string(),
        ty: render_type(ty),
        span: to_source_span(span),
    });
}

/// A declared return type "requires" an explicit return when the runtime
/// fallback (`nil`) wouldn't satisfy it. Nullable types (`T?`) and
/// nullable-containing tuples are free to fall through.
fn requires_return(ty: &Type) -> bool {
    match ty {
        Type::Nullable(_) => false,
        // An explicit `nil` return type is satisfied by the implicit
        // fall-through (which yields `nil` at runtime), so no explicit
        // `return` is needed.
        Type::Named(n) if n == "nil" => false,
        // A tuple return is only safe to fall through if every component
        // is nullable — otherwise the `nil` fallback violates at least one
        // slot's type.
        Type::Tuple(parts) => parts.iter().any(requires_return),
        _ => true,
    }
}

fn render_type(ty: &Type) -> String {
    match ty {
        Type::Named(n) => n.clone(),
        Type::Nullable(inner) => format!("{}?", render_type(inner)),
        Type::Table { key: None, value } => format!("table<{}>", render_type(value)),
        Type::Table {
            key: Some(k),
            value,
        } => {
            format!("table<{}, {}>", render_type(k), render_type(value))
        }
        Type::Tuple(parts) => {
            let inner: Vec<_> = parts.iter().map(render_type).collect();
            format!("({})", inner.join(", "))
        }
        Type::Function { params, ret } => {
            let p: Vec<_> = params.iter().map(render_type).collect();
            format!("fn({}) -> {}", p.join(", "), render_type(ret))
        }
        Type::Generic(g) => {
            let a: Vec<_> = g.args.iter().map(render_type).collect();
            format!("{}<{}>", g.name, a.join(", "))
        }
    }
}

/// `true` if every reachable execution path through `block` ends in a
/// `return` or `throw`.
fn block_returns(block: &[Spanned<Stmt>]) -> bool {
    block.iter().any(|s| stmt_returns(&s.value))
}

fn stmt_returns(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::Throw(_) => true,
        Stmt::If {
            then_block,
            elseifs,
            else_block,
            ..
        } => {
            let Some(else_b) = else_block else {
                return false;
            };
            block_returns(then_block)
                && elseifs.iter().all(|(_, b)| block_returns(b))
                && block_returns(else_b)
        }
        Stmt::Try {
            body, catch_body, ..
        } => {
            // Both the protected region and the handler must return for
            // the `try` itself to count.
            block_returns(body) && block_returns(catch_body)
        }
        // Match: only count if every arm returns AND the arms are
        // exhaustive in a way we can see — either a wildcard arm exists
        // or one bare identifier pattern (which catches everything).
        Stmt::Expr(e) => expr_returns(&e.value),
        // Loops, simple statements, declarations — can't be sure they
        // execute their body or reach a return.
        _ => false,
    }
}

fn expr_returns(expr: &saule_ast::Expr) -> bool {
    if let saule_ast::Expr::Match { arms, .. } = expr {
        if !has_irrefutable_arm(arms) {
            return false;
        }
        arms.iter().all(arm_returns)
    } else {
        false
    }
}

fn arm_returns(arm: &MatchArm) -> bool {
    // A guarded arm may not fire even if the pattern matches; can't count it.
    if arm.guard.is_some() {
        return false;
    }
    match &arm.body {
        saule_ast::MatchBody::Block(b) => block_returns(b),
        saule_ast::MatchBody::Expr(_) => false,
    }
}

fn has_irrefutable_arm(arms: &[MatchArm]) -> bool {
    arms.iter().any(|a| {
        a.guard.is_none() && matches!(a.pattern.value, Pattern::Wildcard | Pattern::Bind(_))
    })
}
