//! Type-parameter bounds: `<T: Named>`.
//!
//! A bound does two jobs, at opposite ends of a generic.
//!
//! Inside the declaration it is what makes a parameter *usable*: a bare
//! `<T>` says only "some type the caller picks", so nothing can be read off
//! a `T` — see [`crate::state::through_bound`], which routes every member
//! lookup on a bounded parameter to the bound instead.
//!
//! At the call site it is an obligation: whatever type the caller picked has
//! to satisfy it. That is this module. Checking it here is what turns the
//! old silent failure — a method call on an unconstrained `T` compiling and
//! blowing up at run time, blaming the generic's body rather than the call
//! that chose the type — into a diagnostic at the call.

use std::collections::HashMap;

use saule_ast::{Type, TypeBound};

use crate::error::TypeCheckError;
use crate::expr::{is_any, named_head, strip_nullable, type_to_string};
use crate::state::{is_interface, is_subtype_named, is_type_param, type_param_bound, with_classes};
use crate::to_source_span;
use saule_sigs::generics::Freshened;

/// Does `found` satisfy `bound`?
pub(crate) fn satisfies(found: &Type, bound: &str) -> bool {
    // A nullable satisfies nothing: a bound promises members, and `nil` has
    // none. The same rule keeps `maybe.name()` from checking.
    if matches!(found, Type::Nullable(_)) {
        return false;
    }
    let Some((name, _)) = named_head(&strip_nullable(found.clone())) else {
        // A `table`, a function type or a tuple implements nothing, since
        // only a class can carry an `implements` clause.
        return false;
    };
    // `any` is the one type that satisfies every bound, for the same reason
    // it satisfies every slot: it is the checker's explicit "I cannot see
    // this", and the `as` cast is where the program says what it is.
    if is_any(&Type::Named(name.clone())) {
        return true;
    }
    // Handing one generic's parameter to another: `fn a<T: Named>(x: T)`
    // calling `fn b<U: Named>(y: U)` with its own `x`. The caller's bound is
    // all that is known about it, so that is what has to imply the callee's.
    if let Some(own) = type_param_bound(&name) {
        return is_subtype_named(&own.name, bound);
    }
    if is_type_param(&name) {
        return false;
    }
    is_subtype_named(&name, bound)
}

/// Check every bound on a call whose type parameters have just been
/// inferred.
///
/// `fresh` is the renaming [`Freshened`] applied to the declaration's
/// parameters and `subst` is what the arguments bound them to; a parameter
/// the arguments never pinned down is absent, and is left alone rather than
/// reported against a guess.
pub(crate) fn check_satisfied(
    bounds: &[TypeBound],
    declared: &[String],
    fresh: &Freshened,
    subst: &HashMap<String, Type>,
    span: std::ops::Range<usize>,
    errors: &mut Vec<TypeCheckError>,
) {
    for b in bounds {
        // A bound naming nothing real is reported once, at the declaration
        // (`BadTypeBound`); repeating it at every call site would bury that.
        if !is_interface(&b.bound.name) && !with_classes(|r| r.contains_key(&b.bound.name)) {
            continue;
        }
        let Some(i) = declared.iter().position(|p| p == &b.param) else {
            continue;
        };
        let Some(key) = fresh.params.get(i) else {
            continue;
        };
        let Some(found) = subst.get(key) else {
            continue;
        };
        if !satisfies(found, &b.bound.name) {
            errors.push(TypeCheckError::BoundNotSatisfied {
                param: b.param.clone(),
                bound: b.bound.name.clone(),
                found: type_to_string(found),
                span: to_source_span(span.clone()),
            });
        }
    }
}
