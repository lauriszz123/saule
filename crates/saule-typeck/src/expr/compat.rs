//! Type compatibility — only flags the cases we can prove are wrong.
//!
//! [`types_compatible`] is the core relation; the `check_*_compat`
//! functions apply it to assignments, table literals and conditions.

use saule_ast::{BinOp, Expr, Spanned, TableEntry, Type};

use crate::TypeCheckError;
use crate::state::{
    Scope, class_implements, is_interface, is_sig_param, is_subtype_named, is_type_param,
};
use crate::to_source_span;

use super::*;

/// Lua's multi-value adjustment: a call returning `(A, B)` used where a
/// single value is expected contributes only `A`. Applied to the *inferred*
/// side of a compatibility check so `local x: integer = pair()` stays legal
/// while `local a, b = pair()` still sees the whole tuple.
pub(crate) fn adjust_to_single(ty: Type) -> Type {
    match ty {
        Type::Tuple(items) => items
            .into_iter()
            .next()
            .unwrap_or(Type::Named("nil".into())),
        other => other,
    }
}

pub(crate) fn check_assignment_compat(
    decl_ty: &Type,
    value: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    // When the value is `lhs ?? rhs`, the declared type tells us what the
    // whole expression is supposed to produce — use its stripped base as
    // the expected type for the fallback. This catches mismatches even
    // when `lhs` has lost type info (e.g. returns `any?`).
    if let Expr::Binary {
        op: BinOp::Coalesce,
        rhs,
        ..
    } = &value.value
    {
        let expected_base = strip_nullable(decl_ty.clone());
        if !matches!(&expected_base, Type::Named(n) if n == "nil" || n == "any")
            && let Some(rt) = infer(rhs, scope)
        {
            let rt_base = strip_nullable(rt.clone());
            let rt_is_nil = matches!(&rt_base, Type::Named(n) if n == "nil");
            let rt_is_any = is_any(&rt_base);
            if !rt_is_nil && !rt_is_any && !types_compatible(&expected_base, &rt_base) {
                errors.push(TypeCheckError::CoalesceFallbackTypeMismatch {
                    expected: type_to_string(&expected_base),
                    found: type_to_string(&rt),
                    span: to_source_span(rhs.span.clone()),
                });
            }
        }
    }

    if is_nullable(decl_ty) {
        // Even for nullable slots we still want to reject obviously
        // incompatible value types — e.g. `local x: string? = some_entry`
        // where `some_entry: Entry?`. Only `nil` stays permissive; an
        // `any` value is a downcast and must go through `as`, even into a
        // nullable slot (`local x: string? = a as string`).
        if let Some(value_ty) = infer(value, scope)
            && !matches!(&value_ty, Type::Named(n) if n == "nil")
            && !types_compatible(decl_ty, &value_ty)
        {
            errors.push(TypeCheckError::AssignmentTypeMismatch {
                expected: type_to_string(decl_ty),
                found: type_to_string(&value_ty),
                span: to_source_span(value.span.clone()),
            });
        }
        return;
    }
    if matches!(value.value, Expr::Nil) {
        errors.push(TypeCheckError::NilToNonNullable {
            ty: type_to_string(decl_ty),
            span: to_source_span(value.span.clone()),
        });
        return;
    }

    // Table-aware checks for table literals assigned to a typed table.
    if check_table_literal_compat(decl_ty, value, scope, errors) {
        return;
    }

    // Strict by construction: an unknown type is an error, never a reason to
    // skip the check. `infer` covers every expression form and answers `any`
    // for genuinely dynamic values, so reaching this arm means the type
    // really could not be worked out — which must not pass silently.
    let Some(value_ty) = infer(value, scope).map(adjust_to_single) else {
        errors.push(TypeCheckError::UndeterminedType {
            span: to_source_span(value.span.clone()),
        });
        return;
    };
    if is_nullable(&value_ty) {
        errors.push(TypeCheckError::NullableToNonNullable {
            from: type_to_string(&value_ty),
            to: type_to_string(decl_ty),
            span: to_source_span(value.span.clone()),
        });
        return;
    }
    // General incompatibility (e.g. `table<Storage>` vs `table<string>`
    // from `Os.args()`). `nil` stays permissive, and so does an `any`
    // *slot* — widening into `any` is always safe. An `any` *value* is
    // not: that is the downcast direction, and it now requires `as`.
    if !matches!(&value_ty, Type::Named(n) if n == "nil")
        && !is_any(decl_ty)
        && !types_compatible(decl_ty, &value_ty)
    {
        errors.push(TypeCheckError::AssignmentTypeMismatch {
            expected: type_to_string(decl_ty),
            found: type_to_string(&value_ty),
            span: to_source_span(value.span.clone()),
        });
    }
}

/// Index key must match the table's declared key type.
pub(crate) fn check_table_key_compat(
    expected: &Type,
    index: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    if let Some(idx_ty) = infer(index, scope)
        && !types_compatible(expected, &idx_ty)
    {
        errors.push(TypeCheckError::TableKeyTypeMismatch {
            expected: type_to_string(expected),
            found: type_to_string(&idx_ty),
            span: to_source_span(index.span.clone()),
        });
    }
}

/// True for `integer` and `any` — the key types that an array-style literal
/// can satisfy.
pub(crate) fn is_integer_like(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if n == "integer" || n == "any")
}

/// True for `string` and `any` — the key types that a field-style literal
/// (`name: expr` / `"text": expr`) can satisfy.
pub(crate) fn is_string_like(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if n == "string" || n == "any")
}

/// Element-of-table compatibility — accepts literals/`Ident`s whose inferred
/// type matches, and stays quiet otherwise (conservative).
/// Check a table *literal* against a declared table type, entry by entry.
/// Returns `true` when the pair was handled — i.e. `expected` is a table
/// type and `value` is a literal — so callers know to skip their general
/// infer-then-compare path.
///
/// Doing this structurally rather than by inference is what makes
/// `{ Positioned() }` fill a `table<Widget>` slot. `table<T>` is invariant
/// (see [`table_part_compatible`]) because a shared mutable container
/// reachable under two element types can be poisoned through the wider
/// name — but a literal at the point of use has no second name to be
/// reached by, so there is nothing for invariance to protect. Inferring it
/// bottom-up as `table<Positioned>` and *then* demanding invariance
/// rejects code that is perfectly sound.
///
/// The literal's positional and field halves are validated separately
/// against the declared shape, so a mismatch points at the offending entry
/// rather than at the whole braces.
pub(crate) fn check_table_literal_compat(
    expected: &Type,
    value: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) -> bool {
    let (
        Type::Table {
            key,
            value: elem_ty,
        },
        Expr::Table(items),
    ) = (expected, &value.value)
    else {
        return false;
    };

    let has_positional = items.iter().any(|e| matches!(e, TableEntry::Positional(_)));
    let has_field = items.iter().any(|e| matches!(e, TableEntry::Field { .. }));

    // `{a, b, c}` literal cannot fill a map-typed table whose key is not
    // integer-compatible.
    if let Some(k) = key
        && !is_integer_like(k)
        && has_positional
    {
        errors.push(TypeCheckError::TableArrayLiteralForMap {
            key: type_to_string(k),
            value: type_to_string(elem_ty),
            span: to_source_span(value.span.clone()),
        });
        return true;
    }
    // Field entries (`name: ...`) require the table to declare a key
    // type compatible with `string`. Array-typed `table<T>` rejects them.
    if has_field {
        let key_ok = match key {
            None => false,
            Some(k) => is_string_like(k),
        };
        if !key_ok {
            errors.push(TypeCheckError::TableArrayLiteralForMap {
                key: key
                    .as_deref()
                    .map(type_to_string)
                    .unwrap_or_else(|| "integer".to_string()),
                value: type_to_string(elem_ty),
                span: to_source_span(value.span.clone()),
            });
            return true;
        }
    }
    // Each value must match the declared value type.
    for item in items {
        match item {
            TableEntry::Positional(e) | TableEntry::Field { value: e, .. } => {
                check_element_compat(elem_ty, e, scope, errors);
            }
        }
    }
    true
}

pub(crate) fn check_element_compat(
    expected: &Type,
    value: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    if let Some(value_ty) = infer(value, scope)
        && !types_compatible(expected, &value_ty)
    {
        errors.push(TypeCheckError::TableElementTypeMismatch {
            expected: type_to_string(expected),
            found: type_to_string(&value_ty),
            span: to_source_span(value.span.clone()),
        });
    }
}

/// Conservative type compatibility — names match (or either is `any`), or
/// `value_ty` is `Nullable` of a compatible inner, etc.
/// Compare one position of a function type — a parameter or the return.
///
/// `any` on either side is permissive: an untyped lambda parameter parses as
/// `any`, and a block-bodied lambda's return type is `any` until the body is
/// analysed, so treating those as mismatches would reject correct code.
pub(crate) fn fn_part_compatible(expected: &Type, found: &Type) -> bool {
    is_any(expected) || is_any(found) || types_compatible(expected, found)
}

/// A table's key type with the array form's implicit key made explicit:
/// `table<T>` means `table<integer, T>`.
pub(crate) fn table_key(key: &Option<Box<Type>>) -> Type {
    key.as_deref()
        .cloned()
        .unwrap_or_else(|| Type::Named("integer".into()))
}

/// Compare one half of a table type — its key type or its value type.
///
/// Tables are mutable, so their parameters are **invariant**: a
/// `table<Dog>` is not a `table<Animal>`. Allowing it would let a write
/// through the wider alias put an `Animal` into a table the other name
/// still believes holds only `Dog`s, and the error would surface far from
/// the assignment that caused it.
///
/// `any`, `nil` and generic type parameters stay permissive. They are the
/// checker's "unknown" sentinels rather than real types — an empty `{}`
/// literal infers `table<any>`, and native signatures use `any` to mean
/// "any table at all".
pub(crate) fn table_part_compatible(expected: &Type, found: &Type) -> bool {
    // An `any` on the **value** side is the untyped-literal case — most
    // importantly the empty `{}`, which has no element type yet and has to
    // be able to fill any table slot.
    //
    // An `any` in the **slot** is not accepted: tables are mutable and
    // shared by reference, so letting `table<integer>` alias as
    // `table<any>` hands out a window through which the container can be
    // poisoned with values its element type forbids.
    if is_any(found) {
        return true;
    }
    if matches!(found, Type::Named(n) if n == "nil") {
        return true;
    }
    types_compatible(expected, found) && types_compatible(found, expected)
}

pub(crate) fn types_compatible(expected: &Type, value_ty: &Type) -> bool {
    // Suppress `is_interface` unused-warning when state moves; reference here.
    let _ = is_interface;
    match (expected, value_ty) {
        // Same-name primitives, plus `any` in the **slot** (widening is
        // always safe), plus `nil` on the value side (nil is universally
        // assignable; nullable-rejection is handled separately by
        // `NullableToNonNullable`).
        //
        // The reverse — an `any` *value* flowing into a concrete slot — is
        // deliberately **not** accepted. That direction is a downcast, and
        // allowing it silently is what used to let `local n: integer = a`
        // put a string in an integer. Write `a as integer` instead: the
        // cast is checked at runtime and yields `integer?`.
        (Type::Named(a), Type::Named(b)) => {
            if a == b || a == "any" || b == "nil" {
                return true;
            }
            // An inference variable from the signature being called into
            // binds to whatever this position supplies — see
            // `compatible_under_sig_params`. Checked before the rigid
            // rule below, since the two sets can share a name.
            if is_sig_param(a) || is_sig_param(b) {
                return true;
            }
            // A type parameter of the body being checked is **rigid**:
            // universally quantified, so it matches only itself.
            //
            // `T` and `U` are independent — `fn map<T, U>(items:
            // table<T>, f: fn(U) -> U)` applying `f` to a `T` is the
            // mistake that catches.
            //
            // A concrete type is no better. `T` stands for whatever the
            // caller picked, so `local n: integer = someT` is a downcast
            // and `local t: T = 0` an upcast, and neither is knowable
            // here. Both used to pass, which made every generic body a
            // hole in the type system: the value flowed on unchecked and
            // failed much later, at the first operation that cared.
            // `any` is *not* affected — it is handled above, so widening
            // a `T` into an `any` slot stays free.
            match (is_type_param(a), is_type_param(b)) {
                (true, true) => return a == b,
                (true, false) | (false, true) => return false,
                (false, false) => {}
            }
            // `number` is the sentinel used in native sigs to mean
            // "integer or float" — accept either.
            if a == "number" && (b == "integer" || b == "float" || b == "number") {
                return true;
            }
            // Class/interface subtyping: a value of type `b` is assignable to
            // a slot of type `a` if `b` is a subtype of `a` (class implements
            // interface, class extends class, interface extends interface).
            if is_subtype_named(b, a) {
                let _ = class_implements; // keep import even if unused later
                return true;
            }
            false
        }
        // Element types are compared through `table_part_compatible`, which
        // permits an untyped value (`{}`) to fill a typed slot but refuses
        // to widen a typed table into `table<any>` — see its comment for
        // why aliasing a mutable container that way is unsound.
        (Type::Table { key: ek, value: ev }, Type::Table { key: vk, value: vv }) => {
            // `table<T>` is the array form — integer-keyed — so it compares
            // against an explicit `table<integer, T>` as the same shape.
            table_part_compatible(&table_key(ek), &table_key(vk)) && table_part_compatible(ev, vv)
        }
        // Expected table, but value is the bare type-name `table`, `any` or
        // `nil` — accept (caller has erased the element type, or it's nil).
        (Type::Table { .. }, Type::Named(n)) if n == "table" || n == "any" || n == "nil" => true,
        // Expected `any` / `table` / `nil` named slot, value is a table —
        // accept (we widen to the named slot).
        (Type::Named(n), Type::Table { .. }) if n == "table" || n == "any" => true,
        // Bare `function` (or `any` / `nil`) named slot vs an actual function
        // value — mirrors the `table` arms. Native sigs erase the precise
        // shape (e.g. an `SFunction` parameter renders as `function`).
        (Type::Function { .. }, Type::Named(n)) if n == "function" || n == "any" || n == "nil" => {
            true
        }
        (Type::Named(n), Type::Function { .. }) if n == "function" || n == "any" => true,
        (Type::Nullable(a), b) => types_compatible(a, b),
        (a, Type::Nullable(b)) => types_compatible(a, b),
        // Function shapes. Parameters are **contravariant** — the value has
        // to accept everything a caller of the declared type may pass it —
        // and the return type is **covariant**: whatever comes back must be
        // usable as the declared return. That is the rule that keeps a
        // call through the declared type correct, and it only ever accepts
        // more than invariance would.
        (
            Type::Function {
                params: ep,
                ret: er,
            },
            Type::Function {
                params: vp,
                ret: vr,
            },
        ) => {
            ep.len() == vp.len()
                && ep
                    .iter()
                    .zip(vp.iter())
                    .all(|(e, v)| fn_part_compatible(v, e))
                && fn_part_compatible(er, vr)
        }
        // Tuple shapes aren't tracked precisely yet; accept rather than
        // emit false positives.
        (Type::Tuple(_), Type::Tuple(_)) => true,
        // Different kinds (e.g. table vs integer) — reject.
        _ => false,
    }
}

/// Reject `if`/`while`/`until` conditions that the type system can prove are
/// not `boolean`. Conservative: when `infer` can't determine a type, we skip
/// silently so calls and dynamic expressions keep working.
pub(crate) fn check_boolean_cond(
    construct: &'static str,
    cond: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let Some(ty) = infer(cond, scope) else {
        return;
    };
    let is_bool = matches!(&ty, Type::Named(n) if n == "boolean" || n == "any");
    if !is_bool {
        errors.push(TypeCheckError::NonBooleanCondition {
            construct,
            found: type_to_string(&ty),
            span: to_source_span(cond.span.clone()),
        });
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Lightweight type inference.
//
// Returns `Some(ty)` only when we can prove the type. Anything we can't see
// through (calls, member reads on unknown classes, indexing) returns `None`,
// and callers treat that as "don't know, don't complain".
// ──────────────────────────────────────────────────────────────────────────────
