//! Flow narrowing.
//!
//! [`narrow_truthy`] applies the assumptions that hold when a condition is
//! true — today `x != nil` and `nil != x` strip `Nullable` off `x`, and
//! `and` chains compose. [`narrow_falsy`] is the else-branch counterpart.

use saule_ast::{BinOp, Expr, Spanned, Type};

use crate::state::Scope;

pub(crate) fn narrow_truthy(cond: &Spanned<Expr>, scope: &mut Scope) {
    match &cond.value {
        Expr::Binary {
            op: BinOp::NotEq,
            lhs,
            rhs,
        } => {
            if let Some(name) = pick_ident_compared_to_nil(lhs, rhs)
                && let Some(Type::Nullable(t)) = scope.lookup(name).cloned()
            {
                scope.bind(name.to_string(), *t);
            }
        }
        Expr::Binary {
            op: BinOp::And,
            lhs,
            rhs,
        } => {
            narrow_truthy(lhs, scope);
            narrow_truthy(rhs, scope);
        }
        _ => {}
    }
}

pub(crate) fn narrow_falsy(cond: &Spanned<Expr>, scope: &mut Scope) {
    if let Expr::Binary {
        op: BinOp::Eq,
        lhs,
        rhs,
    } = &cond.value
        && let Some(name) = pick_ident_compared_to_nil(lhs, rhs)
        && let Some(Type::Nullable(t)) = scope.lookup(name).cloned()
    {
        scope.bind(name.to_string(), *t);
    }
}

/// If `lhs` / `rhs` is `(Ident(x), Nil)` in either order, return `Some(x)`.
pub(crate) fn pick_ident_compared_to_nil<'a>(
    lhs: &'a Spanned<Expr>,
    rhs: &'a Spanned<Expr>,
) -> Option<&'a str> {
    match (&lhs.value, &rhs.value) {
        (Expr::Ident(n), Expr::Nil) => Some(n),
        (Expr::Nil, Expr::Ident(n)) => Some(n),
        _ => None,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Misc helpers.
// ──────────────────────────────────────────────────────────────────────────────
