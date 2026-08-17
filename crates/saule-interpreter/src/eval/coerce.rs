//! `Assignable<T>` — implicit construction at a typed binding site.
//!
//! `local myString: Str = "Hello, world"` builds a `Str` by calling
//! `Str.from("Hello, world")`. The annotation is what selects the target, so
//! this is *target-typed* conversion: there is never a question of which
//! class a bare value should become, only whether the one that was asked for
//! accepts it.
//!
//! ## Where it applies, and why the list is closed
//!
//! Coercion happens exactly where the interpreter can see a declared type:
//!
//! * an annotated `local` / module variable, and
//! * a user function's parameters and declared return type.
//!
//! It deliberately does **not** apply to table elements, field defaults or
//! native arguments. That restriction is not tidiness — it is soundness.
//! `saule-typeck` relaxes its assignment rule at precisely the sites listed
//! above (see `crate::coerce_sites` there); if it relaxed everywhere while
//! this module converted only here, `local t: table<Str> = {"a"}` would
//! typecheck and then leave a raw `string` inside the table for the first
//! `Str` method call to trip over.
//!
//! ## Cost
//!
//! The fast path is one `matches!` on the declared type: only a
//! `Type::Named` naming a class can coerce, and a value that already *is*
//! that class returns untouched. Nothing else pays more than a type-tag
//! comparison.

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::Type;

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::Value;

/// Convert `value` to satisfy `declared`, if the target class opts in with
/// `Assignable` and the value is not already of that class.
///
/// Returns the value unchanged whenever no conversion applies, so callers
/// can wrap a binding unconditionally.
pub fn to_declared(
    value: Value,
    declared: Option<&Type>,
    env: &Rc<RefCell<Environment>>,
    span: &std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let Some(Type::Named(name)) = declared.map(strip_nullable_ref) else {
        return Ok(value);
    };
    // `nil` fills a nullable slot on its own terms; converting it would call
    // `from(nil)` on a signature that almost certainly does not want one.
    if matches!(value, Value::Nil) {
        return Ok(value);
    }
    // Already the right class — the overwhelmingly common case, and the one
    // that has to stay free.
    if let Value::Instance(inst) = &value
        && inst.borrow().class.name == *name
    {
        return Ok(value);
    }
    let Some(Value::Class(class)) = env.borrow().get(name) else {
        return Ok(value);
    };
    let Some(from) = class.lookup_static_method(saule_ast::ops::ASSIGNABLE.method) else {
        return Ok(value);
    };
    let args = [crate::eval::expr::EvaluatedArg::Positional(value)];
    // `from` is a static reached as a plain function, so the tree-walker's
    // arm keeps using `call_function_multi` — statics come through
    // `resolved_owner`, and rerouting it would newly bind `self`.
    let out = match &from {
        crate::value::MethodRef::Tree(f) => {
            crate::eval::expr::call_function_multi(f, &args, span.clone())?
        }
        crate::value::MethodRef::Vm(f) => {
            let [crate::eval::expr::EvaluatedArg::Positional(v)] = &args else {
                unreachable!("built one positional argument just above")
            };
            f.invoke(std::slice::from_ref(v), span.clone())?
        }
    };
    Ok(out.into_iter().next().unwrap_or(Value::Nil))
}

/// The base of a possibly-nullable type, by reference.
///
/// `Str?` and `Str` select the same target: a nullable slot still names the
/// class a non-nil value should become.
fn strip_nullable_ref(ty: &Type) -> &Type {
    match ty {
        Type::Nullable(inner) => strip_nullable_ref(inner),
        other => other,
    }
}
