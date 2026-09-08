//! Generic instantiation: binding a signature's type parameters from
//! actual argument types, and substituting them back out again.

use saule_ast::{Type, TypeArgs};

use crate::state::{pop_sig_params, push_sig_params};

// The pure half of generic instantiation now lives in `saule-sigs`, so
// the native signature table can reach it from upstream of
// `saule-semantic`. Re-exported under the same paths the rest of this
// crate already spells.
pub(crate) use saule_sigs::generics::{
    Freshened, instantiate_param_types, is_any, is_unbound_type_param,
    mentions_unbound_param, substitute, unfreshen_name, unify,
};

use super::*;

/// [`types_compatible`] with the *callee's* type parameters treated as
/// inference variables for the duration of the check.
///
/// A parameter the arguments haven't pinned down yet reaches the
/// comparison as a bare `Named("V")`. Read as an unknown concrete type it
/// matches nothing, and `Table.insert(t, x)` with `t: table<any>` and
/// `x: any` was rejected as "expects `V`, got `any`".
///
/// Scoping the names in makes the *parameter position* permissive without
/// weakening the surrounding structure — `table<V>` still rejects an
/// `integer` argument. The push is kept tight around the comparison so it
/// can't leak into `infer` and shadow a user type that shares the name.
///
/// These go into their own set rather than the body's generics: the two
/// are opposites. A rigid `T` from the enclosing signature is opaque and
/// matches only itself, while a `V` from the callee binds to whatever it
/// is handed. Sharing one set is what made every rigid parameter as
/// permissive as `any`, so `local n: integer = someT` type-checked.
pub(crate) fn compatible_under_sig_params(
    expected: &Type,
    found: &Type,
    params: &[String],
) -> bool {
    if params.is_empty() {
        return types_compatible(expected, found);
    }
    let added = push_sig_params(params);
    let ok = types_compatible(expected, found);
    pop_sig_params(added);
    ok
}

/// Seed a substitution from an explicit `<T, U>` written at the call site.
///
/// Binding here rather than leaving every parameter free is the whole point:
/// [`unify`] refuses to overwrite a name already in `subst`, so a seeded
/// `T := string` survives the walk over the actual arguments and
/// `filter<string>(nums)` on a `table<integer>` is reported as the argument
/// mismatch it is instead of quietly re-inferring `T := integer`.
///
/// A list of the wrong length is reported and then *ignored*, so the call is
/// still checked by inference alone — one complaint about the `<...>` rather
/// than a second, confusing one about every argument.
pub(crate) fn seed_explicit_type_args(
    type_args: Option<&TypeArgs>,
    callee: &str,
    fresh: &Freshened,
    errors: &mut Vec<TypeCheckError>,
) -> std::collections::HashMap<String, Type> {
    let mut subst = std::collections::HashMap::new();
    let Some(ta) = type_args else {
        return subst;
    };
    // Covers the non-generic callee too: it declares no parameters, so any
    // list at all is the wrong length.
    if ta.types.len() != fresh.params.len() {
        errors.push(TypeCheckError::TypeArgArity {
            callee: callee.to_string(),
            expected: fresh.params.len(),
            found: ta.types.len(),
            span: to_source_span(ta.span.clone()),
        });
        return subst;
    }
    for (param, ty) in fresh.params.iter().zip(ta.types.iter()) {
        subst.insert(param.clone(), ty.clone());
    }
    subst
}

