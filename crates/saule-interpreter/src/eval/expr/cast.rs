//! `x as T` — the checked downcast out of `any`.
//!
//! This is what makes `any` sound. Values flow *into* `any` freely, but the
//! only way back out is a runtime type test: [`cast`] returns the value when
//! it really is a `T`, and `nil` otherwise. Since the expression's static
//! type is `T?`, the caller is forced to handle the failure — with `??`, a
//! `nil` check, or `!` to turn it back into a throw.
//!
//! ## Depth
//!
//! `table<T>` is checked **elementwise**: `t as table<integer>` walks every
//! entry and fails if any is not an integer. That is O(n) per cast, and it
//! is the only way the resulting `table<integer>` can be honest — `type()`
//! reports every table as just `"table"`, so a shallow check would hand back
//! a container whose element type is a lie.
//!
//! Recursion is bounded by the *type*, not the value: each step strips one
//! layer off `T`, so a self-referential table can't loop forever. `t[1] = t`
//! checked against `table<integer>` simply fails at the first element.
//!
//! An empty table satisfies any element type vacuously — there is nothing to
//! contradict it, and the alternative (rejecting `{}`) would make the cast
//! useless for freshly-built containers.

use crate::value::Value;
use saule_ast::Type;

/// Perform `value as ty`, yielding `Some(value)` on a match and `None`
/// otherwise. The caller turns `None` into [`Value::Nil`].
pub(crate) fn cast(value: &Value, ty: &Type) -> bool {
    matches_type(value, ty)
}

/// Does `value` actually have type `ty` at runtime?
fn matches_type(value: &Value, ty: &Type) -> bool {
    match ty {
        // `T?` — `nil` passes, otherwise the payload must match `T`.
        Type::Nullable(inner) => matches!(value, Value::Nil) || matches_type(value, inner),

        Type::Named(name) => matches_named(value, name),

        Type::Table { key, value: elem } => match value {
            Value::Table(t) => {
                let t = t.borrow();
                // Array part: keys are integers by construction, so only
                // the element type needs checking. When the declared key
                // type is not integer-compatible, a populated array part
                // contradicts it.
                if !t.array.is_empty()
                    && let Some(k) = key
                    && !is_integer_key_type(k)
                {
                    return false;
                }
                if !t.array.iter().all(|v| matches_type(v, elem)) {
                    return false;
                }
                // Map part: check both halves of every entry.
                t.map.iter().all(|(k, v)| {
                    let key_ok = match key {
                        // `table<T>` is the array form. A populated map
                        // part means it isn't one.
                        None => false,
                        Some(kt) => key_matches(k, kt),
                    };
                    key_ok && matches_type(v, elem)
                })
            }
            _ => false,
        },

        // Any callable satisfies a function type. Parameter and return
        // types are erased at runtime — a `Value::Function` carries no
        // signature we could compare against — so this is deliberately
        // shallow, and the typechecker refuses to hand out a precise
        // `fn(A) -> B` from a cast for exactly that reason.
        Type::Function { .. } => is_callable(value),

        // Tuples only exist as multi-return signatures, never as a value
        // a single expression produces. Nothing can match one.
        Type::Tuple(_) => false,
    }
}

/// Match against a primitive name, `any`, a class name, or an enum name.
fn matches_named(value: &Value, name: &str) -> bool {
    match name {
        // Everything is an `any`. Reachable via `table<any>`, not from a
        // top-level `x as any` (which the typechecker rejects as useless).
        "any" => true,
        "integer" => matches!(value, Value::Int(_)),
        "float" => matches!(value, Value::Float(_)),
        // The sentinel used by native signatures for "integer or float".
        "number" => matches!(value, Value::Int(_) | Value::Float(_)),
        "string" => matches!(value, Value::Str(_)),
        "boolean" => matches!(value, Value::Bool(_)),
        "nil" => matches!(value, Value::Nil),
        "table" => matches!(value, Value::Table(_)),
        "function" => is_callable(value),
        // A user-declared type: an instance of that class (or of any
        // subclass — a `Dog` is an `Animal`), or a variant of that enum.
        other => match value {
            Value::Instance(inst) => {
                let inst = inst.borrow();
                let mut cls = Some(inst.class.clone());
                while let Some(c) = cls {
                    if c.name == other {
                        return true;
                    }
                    cls = c.parent.clone();
                }
                false
            }
            Value::EnumVariant(v) => v.enum_name == other,
            _ => false,
        },
    }
}

/// `true` for every value shape that can appear on the left of a call.
fn is_callable(value: &Value) -> bool {
    matches!(
        value,
        Value::Function(_) | Value::Native(_) | Value::NativeClosure(_)
    )
}

/// Whether a declared table-key type admits integer keys — the array part
/// is integer-keyed, so anything else rules it out.
fn is_integer_key_type(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if n == "integer" || n == "any" || n == "number")
}

/// Check a stored table key against a declared key type.
fn key_matches(key: &crate::value::TableKey, ty: &Type) -> bool {
    use crate::value::TableKey;
    match ty {
        Type::Named(n) if n == "any" => true,
        Type::Named(n) if n == "integer" || n == "number" => matches!(key, TableKey::Int(_)),
        Type::Named(n) if n == "string" => matches!(key, TableKey::Str(_)),
        Type::Named(n) if n == "boolean" => matches!(key, TableKey::Bool(_)),
        Type::Nullable(inner) => key_matches(key, inner),
        _ => false,
    }
}
