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
//! * *what does it evaluate to?* — [`infer_binary`] / [`infer_unary_given`],
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
use super::state::Scope;

// The half of this module that only reads the class registry lives in
// `saule-semantic`, so passes upstream of the checker can consult an
// overload too. Re-exported here because `saule_typeck::ops::…` is the
// path the LSP already spells.
use super::to_source_span;
pub use saule_semantic::ops::{
    class_of, honours, operand_ty, overload_binary_result, overload_unary_result, result_ty,
};

/// The class an operand denotes, when it denotes one.
pub(super) fn operand_class(expr: &Spanned<Expr>, scope: &Scope) -> Option<String> {
    class_of(&infer(expr, scope)?)
}

/// The bound an operand's type parameter was declared with, when the operand
/// is a bounded type parameter.
///
/// `<T: OpAdd<T, T>>` is how a generic says "whatever fills me can be added".
/// There is no class to dispatch on inside the body — `T` is rigid — so the
/// bound is the only thing that can make `a + b` legal there, and the only
/// thing that can say what it evaluates to.
fn operand_bound(expr: &Spanned<Expr>, scope: &Scope) -> Option<saule_ast::TypeRef> {
    bound_of(infer(expr, scope)?)
}

/// [`operand_bound`], for a caller that already holds the operand's type.
fn bound_of(ty: Type) -> Option<saule_ast::TypeRef> {
    let Type::Named(name) = strip_nullable(ty) else {
        return None;
    };
    crate::state::type_param_bound(&name)
}

/// Whether `bound` promises `contract`'s operator.
fn bound_honours(bound: &saule_ast::TypeRef, contract: &OperatorContract) -> bool {
    crate::state::is_subtype_named(&bound.name, contract.interface)
}

/// The type `lhs op rhs` evaluates to when `lhs` is a bounded parameter.
///
/// An operator interface puts its result last — `OpAdd<T, R>`, `OpNeg<R>` —
/// so the bound's final type argument is the answer. A contract that
/// declares no type parameters (`OpLen`) has no argument to read, and its
/// result is fixed by the language rather than by the implementor.
fn bound_result(bound: &saule_ast::TypeRef) -> Option<Type> {
    bound.args.last().cloned()
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
    // A bounded type parameter first: it has no class, so none of the paths
    // below would see it, and the built-in numeric rule would then reject
    // `a + b` with "expected `integer` or `float`" — true of the parameter
    // and useless to the author, who said in the bound what it can do.
    if let Some(bound) = operand_bound(lhs, scope).or_else(|| operand_bound(rhs, scope)) {
        if !bound_honours(&bound, &contract) {
            errors.push(TypeCheckError::OperatorNotInBound {
                op: contract.symbol,
                bound: bound.name.clone(),
                interface: contract.interface,
                span: to_source_span(lhs.span.start..rhs.span.end),
            });
        }
        return true;
    }
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
    if let Some(bound) = operand_bound(rhs, scope) {
        if !bound_honours(&bound, &contract) {
            errors.push(TypeCheckError::OperatorNotInBound {
                op: contract.symbol,
                bound: bound.name.clone(),
                interface: contract.interface,
                span: to_source_span(rhs.span.clone()),
            });
        }
        return true;
    }
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
/// Check `obj[key]` where `obj` is a class instance.
///
/// Two things can be wrong: the class may support no indexing at all, and
/// the key may not be the type `index` declared.
pub(super) fn check_index_receiver(
    obj: &Spanned<Expr>,
    index: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let Some(obj_ty) = infer(obj, scope) else {
        return;
    };
    let base = strip_nullable(obj_ty);
    let Some(class) = class_of(&base) else {
        return;
    };
    if !honours(&class, &saule_ast::ops::OP_INDEX) {
        errors.push(not_implemented(
            &saule_ast::ops::OP_INDEX,
            class,
            obj.span.clone(),
        ));
        return;
    }
    // The key has to be what `index` declared it takes.
    if let Some(sig) = saule_semantic::lookup_method(&class, saule_ast::ops::OP_INDEX.method)
        && let Some(expected) = sig.params.first().map(|p| p.ty.clone())
        && let Some(found) = infer(index, scope)
    {
        let found_base = strip_nullable(found.clone());
        if !is_any(&expected)
            && !is_any(&found_base)
            && !matches!(&found_base, Type::Named(n) if n == "nil")
            && !types_compatible(&expected, &found)
        {
            errors.push(TypeCheckError::OperatorOperandTypeMismatch {
                op: saule_ast::ops::OP_INDEX.symbol,
                class,
                method: saule_ast::ops::OP_INDEX.method,
                expected: type_to_string(&expected),
                found: type_to_string(&found),
                span: to_source_span(index.span.clone()),
            });
        }
    }
}

/// Element type of `obj[key]` when `obj`'s type is a class implementing
/// `OpIndex` — the declared return type of its `index` method.
///
/// `None` for anything else, including a class that declares the interface
/// without defining the method, since [`honours`] wants both.
pub fn index_result(obj_ty: &Type) -> Option<Type> {
    let class = class_of(obj_ty)?;
    honours(&class, &saule_ast::ops::OP_INDEX)
        .then(|| result_ty(&class, &saule_ast::ops::OP_INDEX))?
}

/// Declared key and value types of `obj[key] = value` — the two parameters
/// of `OpNewIndex`'s `newIndex` method.
pub fn new_index_operands(obj_ty: &Type) -> Option<(Type, Type)> {
    let class = class_of(obj_ty)?;
    if !honours(&class, &saule_ast::ops::OP_NEW_INDEX) {
        return None;
    }
    let sig = saule_semantic::lookup_method(&class, saule_ast::ops::OP_NEW_INDEX.method)?;
    let key = sig.params.first()?.ty.clone();
    let value = sig.params.get(1)?.ty.clone();
    Some((key, value))
}

/// Result type of `op rhs` when `rhs`'s **type** resolves to a class that
/// overloads `op`. Scope-free counterpart of [`infer_unary_given`].
/// Result type of `lhs op rhs` when it resolves to an operator overload.
pub(super) fn infer_binary(op: BinOp, lhs: &Spanned<Expr>, scope: &Scope) -> Option<Type> {
    infer_binary_given(op, infer(lhs, scope).as_ref())
}

/// [`infer_binary`] for a caller that already holds the left operand's type.
///
/// Asking twice is not a small waste. An expression `a + b + c` nests to the
/// left, so an arm that infers `lhs` twice runs twice per level, and a sum of
/// 18 terms did not finish checking at all.
pub(super) fn infer_binary_given(op: BinOp, lhs_ty: Option<&Type>) -> Option<Type> {
    // A bounded parameter answers from its bound: `<T: OpAdd<T, R>>` makes
    // `a + b` an `R`. Comparisons are `boolean` however they are implemented
    // and never consult a result type, so they fall through to the built-in
    // rule like any other operand.
    if let Some(contract) = binary_contract(op)
        && !matches!(
            op,
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq
        )
        && let Some(bound) = lhs_ty.cloned().and_then(bound_of)
        && bound_honours(&bound, &contract)
    {
        return bound_result(&bound);
    }
    overload_binary_result(op, lhs_ty?)
}

/// Result type of `op rhs` when it resolves to an operator overload.
pub(super) fn infer_unary_given(op: UnaryOp, rhs_ty: Option<&Type>) -> Option<Type> {
    if let Some(contract) = unary_contract(op)
        && let Some(bound) = rhs_ty.cloned().and_then(bound_of)
        && bound_honours(&bound, &contract)
    {
        return bound_result(&bound);
    }
    overload_unary_result(op, rhs_ty?)
}
