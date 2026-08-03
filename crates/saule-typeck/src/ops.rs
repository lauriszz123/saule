//! Static side of operator overloading.
//!
//! When an operand of `+`, `..`, `<`, `#`, … is a class instance, the
//! built-in numeric/string rules don't apply — the operator means whatever
//! the class's `Op*` interface method says it means. This module answers
//! the two questions the rest of the checker asks about such operands:
//!
//! * *is this operator legal here?* — [`check_binary`] / [`check_unary`],
//!   which report a missing contract and validate the other operand
//!   against the method's declared parameter type;
//! * *what does it evaluate to?* — [`infer_binary`] / [`infer_unary`],
//!   which read the result type straight off the implementing class's own
//!   method signature.
//!
//! Reading the class's signature rather than the interface's type argument
//! keeps this exact without a generic-instantiation pass: a class declaring
//! `implements OpAdd<Vec2, Vec2>` has to define `fn add(other: Vec2) -> Vec2`
//! anyway, and that declaration is already in the class registry.

use saule_ast::ops::{OP_CONCAT, OP_TO_STRING, OperatorContract, binary_contract, unary_contract};
use saule_ast::{BinOp, Expr, Spanned, Type, UnaryOp};

use super::TypeCheckError;
use super::expr::{infer, is_any, strip_nullable, type_to_string, types_compatible};
use super::state::{Scope, class_implements, with_classes};
use super::to_source_span;

/// The class a *type* denotes, when it denotes one. Operator overloading
/// applies to class instances only — a `table`, an `any`, or a primitive
/// keeps the built-in behaviour.
fn class_of(ty: &Type) -> Option<String> {
    let Type::Named(name) = strip_nullable(ty.clone()) else {
        return None;
    };
    with_classes(|reg| reg.contains_key(&name)).then_some(name)
}

/// The class an operand denotes, when it denotes one.
pub(super) fn operand_class(expr: &Spanned<Expr>, scope: &Scope) -> Option<String> {
    class_of(&infer(expr, scope)?)
}

/// Can `class` be the receiver of `contract`'s operator? It must both
/// declare the interface — the opt-in the language asks for — and define
/// the method, which is what dispatch actually calls. Either half may come
/// from a parent class.
fn honours(class: &str, contract: &OperatorContract) -> bool {
    class_implements(class, contract.interface)
        && saule_semantic::lookup_method(class, contract.method).is_some()
}

/// Declared return type of `class`'s contract method, e.g. the `Vec2` in
/// `fn add(other: Vec2) -> Vec2`.
fn result_ty(class: &str, contract: &OperatorContract) -> Option<Type> {
    saule_semantic::lookup_method(class, contract.method)?.return_ty
}

/// Declared type of the contract method's single parameter.
fn operand_ty(class: &str, contract: &OperatorContract) -> Option<Type> {
    saule_semantic::lookup_method(class, contract.method)?
        .params
        .first()
        .map(|p| p.ty.clone())
}

fn not_implemented(
    contract: &OperatorContract,
    class: String,
    span: std::ops::Range<usize>,
) -> TypeCheckError {
    TypeCheckError::OperatorNotImplemented {
        op: contract.symbol,
        class,
        interface: contract.interface,
        method: contract.method,
        span: to_source_span(span),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Checking
// ──────────────────────────────────────────────────────────────────────────────

/// Check `lhs op rhs` when either side is a class instance.
///
/// Returns `true` when this module took ownership of the operator — the
/// caller then skips its numeric/string operand checks, which would only
/// add a redundant "expected `integer` or `float`" next to whatever was
/// reported here.
pub(super) fn check_binary(
    op: BinOp,
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) -> bool {
    let Some(contract) = binary_contract(op) else {
        return false;
    };
    let left = operand_class(lhs, scope);
    let right = operand_class(rhs, scope);
    if left.is_none() && right.is_none() {
        return false;
    }

    match op {
        // Two instances compare by identity when neither side overloads
        // `==`, so a missing `OpEq` is not an error — hand the expression
        // back to the ordinary equality check.
        BinOp::Eq | BinOp::NotEq => {
            let Some((class, other)) = symmetric_receiver(&contract, &left, &right, lhs, rhs)
            else {
                return false;
            };
            check_other_operand(&class, &contract, other, scope, errors);
            true
        }

        // Ordering has no fallback for instances — without `OpCompare`
        // there is nothing to order by.
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            match symmetric_receiver(&contract, &left, &right, lhs, rhs) {
                Some((class, other)) => {
                    check_other_operand(&class, &contract, other, scope, errors)
                }
                None => errors.push(blame(&contract, &left, &right, lhs, rhs)),
            }
            true
        }

        // `..` has two ways to succeed: `OpConcat` builds the result
        // itself, and `OpToString` lets the instance render into an
        // ordinary string concatenation.
        BinOp::Concat => {
            if let Some(class) = left.as_ref().filter(|c| honours(c, &contract)) {
                check_other_operand(class, &contract, rhs, scope, errors);
                return true;
            }
            let stringable = [&left, &right]
                .into_iter()
                .flatten()
                .any(|c| honours(c, &OP_TO_STRING));
            if !stringable {
                errors.push(blame(&OP_CONCAT, &left, &right, lhs, rhs));
            }
            true
        }

        // Arithmetic dispatches on the left operand only, so that
        // `2 - vec` can never quietly become `vec - 2`.
        _ => {
            if let Some(class) = left.as_ref() {
                if honours(class, &contract) {
                    check_other_operand(class, &contract, rhs, scope, errors);
                } else {
                    errors.push(not_implemented(&contract, class.clone(), lhs.span.clone()));
                }
                return true;
            }
            let class = right.expect("one side is a class");
            errors.push(if honours(&class, &contract) {
                TypeCheckError::OperatorDispatchesOnLeft {
                    op: contract.symbol,
                    class,
                    found: infer(lhs, scope)
                        .map(|t| type_to_string(&t))
                        .unwrap_or_else(|| "that value".to_string()),
                    span: to_source_span(lhs.span.start..rhs.span.end),
                }
            } else {
                not_implemented(&contract, class, rhs.span.clone())
            });
            true
        }
    }
}

/// For the symmetric operators, the operand that carries the overload —
/// left first, then right — paired with the expression on the other side.
fn symmetric_receiver<'a>(
    contract: &OperatorContract,
    left: &Option<String>,
    right: &Option<String>,
    lhs: &'a Spanned<Expr>,
    rhs: &'a Spanned<Expr>,
) -> Option<(String, &'a Spanned<Expr>)> {
    if let Some(c) = left.as_ref().filter(|c| honours(c, contract)) {
        return Some((c.clone(), rhs));
    }
    right
        .as_ref()
        .filter(|c| honours(c, contract))
        .map(|c| (c.clone(), lhs))
}

/// "Nobody implements this operator" — reported against the left operand
/// when that's the class, since that's the side dispatch prefers.
fn blame(
    contract: &OperatorContract,
    left: &Option<String>,
    right: &Option<String>,
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
) -> TypeCheckError {
    match (left, right) {
        (Some(c), _) => not_implemented(contract, c.clone(), lhs.span.clone()),
        (_, Some(c)) => not_implemented(contract, c.clone(), rhs.span.clone()),
        _ => unreachable!("called only when one side is a class"),
    }
}

/// Report an operand whose type the contract method won't accept.
fn check_other_operand(
    class: &str,
    contract: &OperatorContract,
    other: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let (Some(expected), Some(found)) = (operand_ty(class, contract), infer(other, scope)) else {
        return;
    };
    let expected_base = strip_nullable(expected.clone());
    let found_base = strip_nullable(found.clone());
    // `any` is the wildcard, and a `nil` operand is nullability's problem.
    if is_any(&expected_base)
        || is_any(&found_base)
        || matches!(&found_base, Type::Named(n) if n == "nil")
    {
        return;
    }
    if !types_compatible(&expected, &found) {
        errors.push(TypeCheckError::OperatorOperandTypeMismatch {
            op: contract.symbol,
            class: class.to_string(),
            method: contract.method,
            expected: type_to_string(&expected),
            found: type_to_string(&found),
            span: to_source_span(other.span.clone()),
        });
    }
}

/// Check `op rhs` when the operand is a class instance. Returns `true`
/// when the operator was handled here.
pub(super) fn check_unary(
    op: UnaryOp,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) -> bool {
    let Some(contract) = unary_contract(op) else {
        return false;
    };
    let Some(class) = operand_class(rhs, scope) else {
        return false;
    };
    if !honours(&class, &contract) {
        errors.push(not_implemented(&contract, class, rhs.span.clone()));
    }
    true
}

// ──────────────────────────────────────────────────────────────────────────────
// Inference
// ──────────────────────────────────────────────────────────────────────────────

/// Result type of `lhs op rhs` when `lhs`'s **type** resolves to a class
/// that overloads `op`. `None` means "not an overload" — the caller's
/// built-in rule applies.
///
/// Only the left operand is consulted: the symmetric operators are
/// `boolean` whoever implements them, and everything else dispatches left.
///
/// This is the type-driven half of [`infer_binary`], split out so callers
/// that already know the operand's type and have no [`Scope`] — the LSP's
/// hover and inlay walkers — apply exactly the same rule instead of
/// re-deriving a weaker one.
pub fn overload_binary_result(op: BinOp, lhs_ty: &Type) -> Option<Type> {
    let contract = binary_contract(op)?;
    if matches!(
        op,
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq
    ) {
        return None;
    }
    let class = class_of(lhs_ty)?;
    honours(&class, &contract).then(|| result_ty(&class, &contract))?
}

/// Result type of `op rhs` when `rhs`'s **type** resolves to a class that
/// overloads `op`. Scope-free counterpart of [`infer_unary`].
pub fn overload_unary_result(op: UnaryOp, rhs_ty: &Type) -> Option<Type> {
    let contract = unary_contract(op)?;
    let class = class_of(rhs_ty)?;
    honours(&class, &contract).then(|| result_ty(&class, &contract))?
}

/// Result type of `lhs op rhs` when it resolves to an operator overload.
pub(super) fn infer_binary(op: BinOp, lhs: &Spanned<Expr>, scope: &Scope) -> Option<Type> {
    overload_binary_result(op, &infer(lhs, scope)?)
}

/// Result type of `op rhs` when it resolves to an operator overload.
pub(super) fn infer_unary(op: UnaryOp, rhs: &Spanned<Expr>, scope: &Scope) -> Option<Type> {
    overload_unary_result(op, &infer(rhs, scope)?)
}
