//! Flow narrowing.
//!
//! [`narrow_truthy`] applies the assumptions that hold when a condition is
//! true — `x != nil` and `nil != x` strip `Nullable` off `x`, a truthy
//! bare `x` does the same, and `and` chains compose. [`narrow_falsy`] is
//! the else-branch counterpart, covering `x == nil`. `not` flips between
//! the two, which is what makes the `if not x then return end` guard work.
//!
//! Both directions deliberately handle only *identifiers* — narrowing a
//! field or index would have to track invalidation on every write.

use saule_ast::{BinOp, Expr, Spanned, Type, UnaryOp};

use crate::state::Scope;

pub(crate) fn narrow_truthy(cond: &Spanned<Expr>, scope: &mut Scope) {
    match &cond.value {
        Expr::Binary {
            op: BinOp::NotEq,
            lhs,
            rhs,
        } => {
            if let Some(name) = pick_ident_compared_to_nil(lhs, rhs) {
                strip_nullable_binding(name, scope);
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
        // A truthy `x` cannot be nil. Conditions must be `boolean`, so
        // this arm is reached through `not x` rather than from a bare
        // `if x then` — see the `Unary` arm in `narrow_falsy`.
        Expr::Ident(name) => strip_nullable_binding(name, scope),
        // `if not x then A else B end` — the else branch knows `x` is
        // truthy, so the inner condition narrows the opposite way.
        Expr::Unary {
            op: UnaryOp::Not,
            rhs,
        } => narrow_falsy(rhs, scope),
        _ => {}
    }
}

pub(crate) fn narrow_falsy(cond: &Spanned<Expr>, scope: &mut Scope) {
    match &cond.value {
        Expr::Binary {
            op: BinOp::Eq,
            lhs,
            rhs,
        } => {
            if let Some(name) = pick_ident_compared_to_nil(lhs, rhs) {
                strip_nullable_binding(name, scope);
            }
        }
        // `if not x then return end` — falling through means `not x` was
        // false, i.e. `x` is truthy and therefore non-nil. This is the
        // guard idiom the `== nil` form already supported.
        Expr::Unary {
            op: UnaryOp::Not,
            rhs,
        } => narrow_truthy(rhs, scope),
        _ => {}
    }
}

/// Rebind `name` to its non-nullable inner type, if it is a nullable local.
/// A no-op for unknown or already non-nullable names.
fn strip_nullable_binding(name: &str, scope: &mut Scope) {
    if let Some(Type::Nullable(t)) = scope.lookup(name).cloned() {
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
