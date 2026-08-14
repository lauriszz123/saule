//! Operator operand checking: the numeric / concat / ordering rules for
//! binary operators, and the `??` fallback and equality compatibility
//! checks. Overloaded operators are handled by [`crate::ops`].

use saule_ast::{BinOp, Expr, Spanned, Type};

use crate::TypeCheckError;
use crate::state::Scope;
use crate::to_source_span;

use super::*;

/// Reject `lhs ?? rhs` when the fallback's type is incompatible with the
/// stripped base type of the left-hand side. Stays conservative: if either
/// side's type can't be inferred, or either side is `any`, the check is
/// skipped (matches how the rest of the typechecker handles `any`).
pub(crate) fn check_coalesce_fallback(
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let (Some(lt), Some(rt)) = (infer(lhs, scope), infer(rhs, scope)) else {
        return;
    };
    let base = strip_nullable(lt);
    // `nil` fallback collapses to `T?` and is always fine.
    if matches!(&rt, Type::Named(n) if n == "nil") {
        return;
    }
    // Literal-`nil` lhs (or any expression typed exactly as `nil`) is a
    // degenerate `??` — the fallback is always taken, so any type is
    // valid. Don't second-guess it.
    if matches!(&base, Type::Named(n) if n == "nil") {
        return;
    }
    // Don't flag when either side is `any` — that's the explicit escape
    // hatch; flagging would just spam diagnostics in dynamic code.
    if is_any(&base) || is_any(&strip_nullable(rt.clone())) {
        return;
    }
    if !types_compatible(&base, &strip_nullable(rt.clone())) {
        errors.push(TypeCheckError::CoalesceFallbackTypeMismatch {
            expected: type_to_string(&base),
            found: type_to_string(&rt),
            span: to_source_span(rhs.span.clone()),
        });
    }
}

/// True for numeric scalars: `integer`, `float`, the `number` sentinel, and
/// `any` (which acts as a wildcard).
pub(crate) fn is_numeric_like(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if n == "integer" || n == "float" || n == "number" || n == "any")
}

/// True for operands of the bitwise operators.
///
/// Stricter than [`is_numeric_like`]: a bit pattern is a property of an
/// `integer`, and Lua's "float with no fractional part is accepted" rule
/// has no place in a language where `integer` and `float` never mix
/// implicitly anywhere else. `number` is the sentinel for a numeric whose
/// kind is not pinned down and `any` is the wildcard, so both pass — same
/// conservatism the arithmetic rules use.
pub(crate) fn is_bitwise_like(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if n == "integer" || n == "number" || n == "any")
}

/// True for operands of `..` (string concatenation). Saule follows Lua and
/// coerces numbers to strings, so all numeric types are accepted too.
pub(crate) fn is_concat_like(ty: &Type) -> bool {
    is_string_like(ty) || is_numeric_like(ty)
}

/// Friendly printable name of a binary operator, used in diagnostics.
pub(crate) fn binop_name(op: BinOp) -> &'static str {
    saule_ast::ops::binop_symbol(op)
}

/// Per-operator type validation. Each branch checks just enough to flag
/// obvious mistakes (string + integer, table < 5, "x" and y, etc.) while
/// staying conservative for `any`, `nil`, and uninferable expressions.
pub(crate) fn check_binary_op(
    op: BinOp,
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    // A class instance on either side means the operator is whatever that
    // class's `Op*` overload says it is; the numeric/string rules below
    // don't apply and would only pile on a confusing second error.
    if crate::ops::check_binary(op, lhs, rhs, scope, errors) {
        return;
    }
    match op {
        BinOp::Coalesce => check_coalesce_fallback(lhs, rhs, scope, errors),
        BinOp::Eq | BinOp::NotEq => check_equality_compat(op, lhs, rhs, scope, errors),
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow => {
            let name = binop_name(op);
            check_operand_kind(
                name,
                "`integer` or `float`",
                is_numeric_like,
                lhs,
                scope,
                errors,
            );
            check_operand_kind(
                name,
                "`integer` or `float`",
                is_numeric_like,
                rhs,
                scope,
                errors,
            );
            check_numeric_kinds_match(lhs, rhs, scope, errors);
        }
        // `&` `|` `~` `<<` `>>` — integers on both sides, always. A shift
        // count is an ordinary `integer` operand too, so the same check
        // covers both sides of all five.
        BinOp::BAnd | BinOp::BOr | BinOp::BXor | BinOp::Shl | BinOp::Shr => {
            let name = binop_name(op);
            check_operand_kind(name, "`integer`", is_bitwise_like, lhs, scope, errors);
            check_operand_kind(name, "`integer`", is_bitwise_like, rhs, scope, errors);
        }
        BinOp::And | BinOp::Or => {
            // Saule follows Lua semantics for `and`/`or`: they short-circuit
            // on truthiness and accept any value (`nil or "x"` → `"x"`),
            // so the operands aren't type-restricted.
        }
        BinOp::Concat => {
            check_operand_kind(
                "..",
                "`string` or numeric",
                is_concat_like,
                lhs,
                scope,
                errors,
            );
            check_operand_kind(
                "..",
                "`string` or numeric",
                is_concat_like,
                rhs,
                scope,
                errors,
            );
        }
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            check_ordering_operands(op, lhs, rhs, scope, errors);
        }
    }
}

/// Check that a single operand satisfies a predicate; emit if not. Stays
/// silent when inference fails, the operand is `any`, or the operand is
/// literally `nil`.
pub(crate) fn check_operand_kind(
    op: &'static str,
    expected: &'static str,
    pred: impl Fn(&Type) -> bool,
    arg: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let Some(t) = infer(arg, scope) else {
        return;
    };
    let base = strip_nullable(t.clone());
    if is_any(&base) {
        return;
    }
    if matches!(&base, Type::Named(n) if n == "nil") {
        return;
    }
    // A `T?` operand is rejected before the kind test, because the kind test
    // runs on the *stripped* base and would otherwise wave `maybeName .. "!"`
    // through on the strength of the `string` inside the `string?`.
    //
    // Null safety is enforced on assignment and at call sites; operators were
    // the gap, which meant `count + 1` on an `integer?` type-checked and then
    // failed at runtime.
    //
    // Deliberately limited to *identifiers*. `narrow` (see `expr::narrow`)
    // only tracks identifiers, so a bare name is exactly the case where the
    // checker can also see the `if x != nil` that makes the operand safe —
    // the diagnostic is therefore always actionable. Extending it to fields
    // would reject the common and correct
    //
    //     if self.dueDate == nil then return end
    //     return Os.time() >= self.dueDate
    //
    // because field narrowing does not exist, leaving `!` or a local copy as
    // the only way out. Requiring that is a language-design decision (it is
    // the rule Kotlin picked for mutable properties) and needs field
    // narrowing built first, so it is not made here.
    if is_nullable(&t) && matches!(arg.value, Expr::Ident(_)) {
        errors.push(TypeCheckError::NullableOperand {
            op,
            found: type_to_string(&t),
            span: to_source_span(arg.span.clone()),
        });
        return;
    }
    if !pred(&base) {
        errors.push(TypeCheckError::BinaryOperandTypeMismatch {
            op,
            expected,
            found: type_to_string(&t),
            span: to_source_span(arg.span.clone()),
        });
    }
}

/// `<`, `<=`, `>`, `>=` require both sides to be in the same family —
/// numeric/numeric or string/string. Anything else is flagged on the side
/// that breaks the family.
pub(crate) fn check_ordering_operands(
    op: BinOp,
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let name = binop_name(op);
    // First, each side individually must be orderable (numeric or string).
    check_operand_kind(
        name,
        "`integer`, `float`, or `string`",
        is_orderable,
        lhs,
        scope,
        errors,
    );
    check_operand_kind(
        name,
        "`integer`, `float`, or `string`",
        is_orderable,
        rhs,
        scope,
        errors,
    );
    // Then, both sides must agree on the family.
    let (Some(lt), Some(rt)) = (infer(lhs, scope), infer(rhs, scope)) else {
        return;
    };
    let lb = strip_nullable(lt.clone());
    let rb = strip_nullable(rt.clone());
    if is_any(&lb) || is_any(&rb) {
        return;
    }
    if matches!(&lb, Type::Named(n) if n == "nil") || matches!(&rb, Type::Named(n) if n == "nil") {
        return;
    }
    let l_num = is_numeric_like(&lb);
    let r_num = is_numeric_like(&rb);
    let l_str = is_string_like(&lb);
    let r_str = is_string_like(&rb);
    let same_family = (l_num && r_num) || (l_str && r_str);
    if !same_family && (l_num || l_str) && (r_num || r_str) {
        // Both individually orderable but in different families — flag rhs
        // with the lhs family as the expected one.
        let expected = if l_num {
            "`integer` or `float`"
        } else {
            "`string`"
        };
        errors.push(TypeCheckError::BinaryOperandTypeMismatch {
            op: name,
            expected,
            found: type_to_string(&rt),
            span: to_source_span(rhs.span.clone()),
        });
    }
}

pub(crate) fn is_orderable(ty: &Type) -> bool {
    is_numeric_like(ty) || is_string_like(ty)
}

/// Saule is strict on numeric kinds — `integer` and `float` never mix
/// implicitly. When both operands have a known *concrete* numeric kind
/// (i.e. not the `number` sentinel, not `any`, not generic), flag the
/// pair when they disagree. The error mirrors `RuntimeError::NumericMix`
/// so the user sees the same diagnostic at compile time.
pub(crate) fn check_numeric_kinds_match(
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let (Some(lt), Some(rt)) = (infer(lhs, scope), infer(rhs, scope)) else {
        return;
    };
    let lb = strip_nullable(lt);
    let rb = strip_nullable(rt);
    let kind = |t: &Type| -> Option<&'static str> {
        if let Type::Named(n) = t {
            match n.as_str() {
                "integer" => Some("integer"),
                "float" => Some("float"),
                _ => None,
            }
        } else {
            None
        }
    };
    let (Some(lk), Some(rk)) = (kind(&lb), kind(&rb)) else {
        return;
    };
    if lk != rk {
        // Span covers both sides so the underline brackets the whole
        // expression — matches the runtime diagnostic.
        let span_start = lhs.span.start.min(rhs.span.start);
        let span_end = lhs.span.end.max(rhs.span.end);
        errors.push(TypeCheckError::NumericMix {
            span: to_source_span(span_start..span_end),
        });
    }
}

/// Reject equality comparisons whose two sides have provably-disjoint
/// types (e.g. comparing a `table?` value to a string literal). Skips when
/// either side involves `any` or `nil`, since `T? == nil` is the idiomatic
/// nullability check.
pub(crate) fn check_equality_compat(
    op: BinOp,
    lhs: &Spanned<Expr>,
    rhs: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let (Some(lt), Some(rt)) = (infer(lhs, scope), infer(rhs, scope)) else {
        return;
    };
    let lb = strip_nullable(lt.clone());
    let rb = strip_nullable(rt.clone());
    // `nil` on either side is legitimate — `x == nil` is how you check
    // nullability.
    if matches!(&lb, Type::Named(n) if n == "nil") || matches!(&rb, Type::Named(n) if n == "nil") {
        return;
    }
    if is_any(&lb) || is_any(&rb) {
        return;
    }
    // Compatible in either direction → OK.
    if types_compatible(&lb, &rb) || types_compatible(&rb, &lb) {
        return;
    }
    let result = if matches!(op, BinOp::Eq) {
        "false"
    } else {
        "true"
    };
    // Span covers both sides.
    let span_start = lhs.span.start.min(rhs.span.start);
    let span_end = lhs.span.end.max(rhs.span.end);
    errors.push(TypeCheckError::DisjointEquality {
        left: type_to_string(&lt),
        right: type_to_string(&rt),
        result,
        span: to_source_span(span_start..span_end),
    });
}
