//! `x as T` — the checked downcast out of `any`, and the conversion.
//!
//! Two operations, one keyword; which one a given `as` is was settled by
//! the typechecker and travels in the node's [`CastKind`]. [`cast`] is the
//! test, [`convert`] is the conversion. They meet the same values — a
//! `3.9` is `nil` to the test and `3` to the conversion — which is exactly
//! why the decision could not be left until now.
//!
//! The test is what makes `any` sound. Values flow *into* `any` freely,
//! but the only way back out is a runtime type test: [`cast`] returns the
//! value when it really is a `T`, and `nil` otherwise. Since the
//! expression's static type is `T?`, the caller is forced to handle the
//! failure — with `??`, a `nil` check, or `!` to turn it back into a throw.
//!
//! ## Depth (the test)
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

use crate::value::{SauleStr, Value};
use saule_ast::Type;

/// Perform `value as ty`, yielding `Some(value)` on a match and `None`
/// otherwise. The caller turns `None` into [`Value::Nil`].
pub fn cast(value: &Value, ty: &Type) -> bool {
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
        "string" => matches!(value, Value::Str(_)),
        "boolean" => matches!(value, Value::Bool(_)),
        "nil" => matches!(value, Value::Nil),
        "table" => matches!(value, Value::Table(_)),
        // No `"function"` arm: the bare name is not a type. A callable is
        // narrowed with the `fn(...) -> T` form, handled above.
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
    matches!(ty, Type::Named(n) if n == "integer" || n == "any")
}

/// Check a stored table key against a declared key type.
fn key_matches(key: &crate::value::TableKey, ty: &Type) -> bool {
    use crate::value::TableKey;
    match ty {
        Type::Named(n) if n == "any" => true,
        Type::Named(n) if n == "integer" => matches!(key, TableKey::Int(_)),
        Type::Named(n) if n == "string" => matches!(key, TableKey::Str(_)),
        Type::Named(n) if n == "boolean" => matches!(key, TableKey::Bool(_)),
        Type::Nullable(inner) => key_matches(key, inner),
        _ => false,
    }
}

/// Perform a converting `value as ty`.
///
/// Only reached on a node the typechecker resolved to
/// [`CastKind::Convert`](saule_ast::CastKind::Convert), so `ty` is one of
/// the pairs `saule_typeck::casts` admits and the arms below cover them
/// all. Returns [`Value::Nil`] for the conversions that are allowed to
/// fail — a string that holds no number — which is why those are typed
/// `T?` and the total ones are not.
pub fn convert(value: &Value, ty: &Type) -> Value {
    // `x as T?` converts the same way `x as T` does; the `?` only shows up
    // in the static type. And `nil` converts to `nil` whatever the target:
    // a cast off a nullable value preserves the absence rather than
    // inventing a `0` for it.
    let ty = match ty {
        Type::Nullable(inner) => inner.as_ref(),
        other => other,
    };
    if matches!(value, Value::Nil) {
        return Value::Nil;
    }

    let Type::Named(name) = ty else {
        return Value::Nil;
    };

    match (name.as_str(), value) {
        // Truncation toward zero, saturating at the `i64` ends and
        // mapping NaN to `0` — Rust's `as` on the float, and the same
        // answer the removed `int()` prelude function gave for every
        // one of those inputs.
        ("integer", Value::Float(f)) => Value::Int(f.trunc() as i64),
        ("integer", Value::Int(n)) => Value::Int(*n),
        ("float", Value::Int(n)) => Value::Float(*n as f64),
        ("float", Value::Float(f)) => Value::Float(*f),

        // Parsing: the fallible direction. Surrounding space is trimmed
        // because a string read from input usually carries some.
        ("integer", Value::Str(s)) => match s.trim().parse::<i64>() {
            Ok(n) => Value::Int(n),
            Err(_) => Value::Nil,
        },
        ("float", Value::Str(s)) => match s.trim().parse::<f64>() {
            Ok(f) => Value::Float(f),
            Err(_) => Value::Nil,
        },

        // Rendering. `to_display_string` is what `print` and `..` use, so
        // `n as string` and `"" .. n` agree — including the `2.0` that
        // keeps a float visibly a float.
        ("string", v @ (Value::Int(_) | Value::Float(_) | Value::Bool(_))) => {
            Value::Str(SauleStr::new(v.to_display_string()))
        }
        ("string", Value::Str(s)) => Value::Str(s.clone()),

        _ => Value::Nil,
    }
}
