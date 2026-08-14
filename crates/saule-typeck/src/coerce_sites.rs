//! Where `Assignable<T>` conversion is allowed to apply.
//!
//! `Assignable` relaxes the assignment rule: a `string` may fill a `Str` slot
//! when `Str` declares `static fn from(s: string) -> Str`. The relaxation is
//! **not** global, and this module exists to make that fact hard to lose.
//!
//! The interpreter converts only where it can see a declared type — an
//! annotated `local` or module variable, and a user function's parameters
//! and return type (`saule_interpreter::eval::coerce`). If the checker
//! relaxed everywhere instead, the two would disagree and the difference
//! would be unsound rather than merely inconsistent:
//!
//! ```text
//! local t: table<Str> = {"a"}     -- would typecheck…
//! t[1].upper()                    -- …and find a raw `string` at runtime
//! ```
//!
//! So [`accepts`] is called at the coercing sites and plain
//! [`crate::expr::types_compatible`] everywhere else. Adding a site means
//! adding it in both crates, and the fixtures in `tests/ui` pin the
//! non-coercing ones.

use saule_ast::Type;

use crate::state::{class_implements, with_classes};

/// Is `found` assignable to `expected` **at a coercing site** — either
/// outright, or by building it through `Assignable`?
pub(crate) fn accepts(expected: &Type, found: &Type) -> bool {
    crate::expr::types_compatible(expected, found) || from_contract_accepts(expected, found)
}

/// The class a *type* denotes, when it denotes a user-declared one.
fn class_of(ty: &Type) -> Option<String> {
    let Type::Named(name) = crate::expr::strip_nullable(ty.clone()) else {
        return None;
    };
    with_classes(|reg| reg.contains_key(&name)).then_some(name)
}

/// Does the class named by `expected` accept a `found` through `Assignable`?
///
/// The one relaxation of the assignment rule, applied only from [`accepts`]
/// so the closed set of sites above stays closed.
fn from_contract_accepts(expected: &Type, found: &Type) -> bool {
    let Some(class) = class_of(expected) else {
        return false;
    };
    if !class_implements(&class, saule_ast::ops::ASSIGNABLE.interface) {
        return false;
    }
    let Some(sig) = saule_semantic::lookup_method(&class, saule_ast::ops::ASSIGNABLE.method) else {
        return false;
    };
    // The contract is static — an instance method named `from` is a
    // different thing and must not silently become a conversion.
    if !sig.is_static {
        return false;
    }
    let Some(param) = sig.params.first() else {
        return false;
    };
    crate::expr::types_compatible(&param.ty, found)
}
