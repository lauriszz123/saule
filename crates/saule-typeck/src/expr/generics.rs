//! Generic instantiation: binding a signature's type parameters from
//! actual argument types, and substituting them back out again.

use saule_ast::Type;

use crate::state::{pop_generics, push_generics};

use super::*;

/// True when `ty` is a `Named(n)` where `n` is one of the (still-unbound)
/// type parameters from the surrounding signature. Such a slot can bind
/// to any concrete type — including nullable ones — so the targeted
/// nullable-into-non-nullable rejection should not fire for it.
pub(crate) fn is_unbound_type_param(ty: &Type, params: &[String]) -> bool {
    matches!(ty, Type::Named(n) if params.iter().any(|p| p == n))
}

/// [`types_compatible`] with the *callee's* type parameters treated as
/// `any`-equivalent for the duration of the check.
///
/// `types_compatible` recognises type parameters through [`is_type_param`],
/// which reads the generics currently in scope — and that set is populated
/// only for the user *body* being checked, never for the signature being
/// called into. So a parameter the arguments hadn't pinned down yet reached
/// the comparison as a bare `Named("V")` and was treated as an unknown
/// concrete type, which matches nothing: `Table.insert(t, x)` with
/// `t: table<any>` and `x: any` was rejected as "expects `V`, got `any`".
///
/// Scoping the names in makes the *parameter position* permissive without
/// weakening the surrounding structure — `table<V>` still rejects an
/// `integer` argument. The push is kept tight around the comparison so it
/// can't leak into `infer` and shadow a user type that shares the name.
pub(crate) fn compatible_under_sig_params(
    expected: &Type,
    found: &Type,
    params: &[String],
) -> bool {
    if params.is_empty() {
        return types_compatible(expected, found);
    }
    let added = push_generics(params);
    let ok = types_compatible(expected, found);
    pop_generics(added);
    ok
}

/// Substitute bound type variables in `ty` with their concrete types from
/// `subst`. Unbound variables (and non-parameter names) are returned as-is.
pub(crate) fn substitute(
    ty: &Type,
    subst: &std::collections::HashMap<String, Type>,
    params: &[String],
) -> Type {
    match ty {
        Type::Named(n) if params.iter().any(|p| p == n) => {
            subst.get(n).cloned().unwrap_or_else(|| ty.clone())
        }
        Type::Named(_) => ty.clone(),
        Type::Nullable(inner) => Type::Nullable(Box::new(substitute(inner, subst, params))),
        Type::Table { key, value } => Type::Table {
            key: key.as_ref().map(|k| Box::new(substitute(k, subst, params))),
            value: Box::new(substitute(value, subst, params)),
        },
        Type::Tuple(items) => {
            Type::Tuple(items.iter().map(|t| substitute(t, subst, params)).collect())
        }
        Type::Function { params: ps, ret } => Type::Function {
            params: ps.iter().map(|t| substitute(t, subst, params)).collect(),
            ret: Box::new(substitute(ret, subst, params)),
        },
    }
}

/// Whether `ty` still mentions any of `params` after substitution — i.e. a
/// type parameter the call's arguments never pinned down. Such a type is
/// unknown, not concrete, so callers treat it as uninferable rather than
/// letting the bare parameter name escape as if it were a real type.
pub(crate) fn mentions_unbound_param(ty: &Type, params: &[String]) -> bool {
    match ty {
        Type::Named(n) => params.iter().any(|p| p == n),
        Type::Nullable(inner) => mentions_unbound_param(inner, params),
        Type::Table { key, value } => {
            key.as_ref()
                .is_some_and(|k| mentions_unbound_param(k, params))
                || mentions_unbound_param(value, params)
        }
        Type::Tuple(items) => items.iter().any(|t| mentions_unbound_param(t, params)),
        Type::Function { params: ps, ret } => {
            ps.iter().any(|t| mentions_unbound_param(t, params))
                || mentions_unbound_param(ret, params)
        }
    }
}

/// One-way unification: bind type-param names in `expected` to corresponding
/// concrete shapes from `found`. Conservative — if shapes don't line up,
/// silently skip (the regular compatibility check will surface the mismatch).
pub(crate) fn unify(
    expected: &Type,
    found: &Type,
    params: &[String],
    subst: &mut std::collections::HashMap<String, Type>,
) {
    // A free type-param on the expected side binds to whatever the actual
    // argument's type is — **including its nullability**. `Table.insert`
    // is `insert<V>(table<V>, V)`, so a `table<any?>` first argument has
    // to bind `V := any?`; stripping the `Nullable` here would bind
    // `V := any`, and the element type could never be nullable at all.
    if let Type::Named(n) = expected
        && params.iter().any(|p| p == n)
        && !subst.contains_key(n)
    {
        // Don't bind `V := any` — that would erase the constraint for the
        // remaining args. Leave it unbound so later args can refine it.
        // `any?` is a real constraint though (it permits nil), so it binds.
        if !is_any(found) {
            subst.insert(n.clone(), found.clone());
        }
        return;
    }
    match (expected, found) {
        (Type::Nullable(e_inner), Type::Nullable(f_inner)) => {
            unify(e_inner, f_inner, params, subst);
        }
        (Type::Nullable(e_inner), other) => unify(e_inner, other, params, subst),
        (
            Type::Table {
                value: e_val,
                key: e_key,
            },
            Type::Table {
                value: f_val,
                key: f_key,
            },
        ) => {
            unify(e_val, f_val, params, subst);
            if let (Some(ek), Some(fk)) = (e_key, f_key) {
                unify(ek, fk, params, subst);
            }
        }
        (Type::Tuple(es), Type::Tuple(fs)) if es.len() == fs.len() => {
            for (e, f) in es.iter().zip(fs.iter()) {
                unify(e, f, params, subst);
            }
        }
        (
            Type::Function {
                params: ep,
                ret: er,
            },
            Type::Function {
                params: fp,
                ret: fr,
            },
        ) if ep.len() == fp.len() => {
            for (e, f) in ep.iter().zip(fp.iter()) {
                unify(e, f, params, subst);
            }
            unify(er, fr, params, subst);
        }
        _ => {}
    }
}

pub(crate) fn is_any(t: &Type) -> bool {
    matches!(t, Type::Named(n) if n == "any")
}
