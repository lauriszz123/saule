//! Generic instantiation: binding a signature's type parameters from
//! actual argument types, and substituting them back out again.
//!
//! Pure functions over [`Type`], with no typechecker state behind them.
//! They live here rather than in `saule-typeck` because the native
//! signature table next door is what needs them, and that table has to sit
//! upstream of `saule-semantic` — see this crate's own docs. The two
//! helpers that *do* need typechecker state (`compatible_under_sig_params`,
//! which scopes parameter names into the compatibility check, and
//! `seed_explicit_type_args`, which reports a diagnostic) stay behind in
//! `saule_typeck::expr::generics`.

use saule_ast::Type;

/// True when `ty` is a `Named(n)` where `n` is one of the (still-unbound)
/// type parameters from the surrounding signature. Such a slot can bind
/// to any concrete type — including nullable ones — so the targeted
/// nullable-into-non-nullable rejection should not fire for it.
pub fn is_unbound_type_param(ty: &Type, params: &[String]) -> bool {
    matches!(ty, Type::Named(n) if params.iter().any(|p| p == n))
}

/// Suffix marking a type-parameter name as freshened. `$` is not an
/// identifier byte in the lexer, so no name written in source — and no
/// type name a native registers — can ever collide with one of these.
const FRESH_MARKER: &str = "$";

/// Strip the freshening suffix so diagnostics quote the parameter the
/// way the signature spells it. Applied at the single rendering point
/// rather than at each error site, so a fresh name can never reach the
/// user however it got there.
pub fn unfreshen_name(name: &str) -> &str {
    name.strip_suffix(FRESH_MARKER).unwrap_or(name)
}

/// A callee's type parameters, renamed apart from everything in the
/// caller's scope.
///
/// Two signatures that both spell a parameter `T` are still two
/// different types, and one of them may be *rigid* — a `T` belonging to
/// the function whose body we are checking. Comparing by name alone
/// cannot tell them apart, so the callee's `T` made the caller's `T`
/// permissive for the duration of the check, and
/// `g(myT)` against `fn g<T>(n: integer)` slipped through.
///
/// Renaming happens once, up front: every later `unify` / `substitute` /
/// compatibility step then works in fresh space, where a leftover
/// parameter name can only be the callee's.
pub struct Freshened {
    /// The renamed parameters, to hand to [`unify`] and friends.
    pub params: Vec<String>,
    originals: Vec<String>,
    renames: std::collections::HashMap<String, Type>,
}

impl Freshened {
    pub fn new(type_params: &[String]) -> Self {
        let params: Vec<String> = type_params
            .iter()
            .map(|p| format!("{p}{FRESH_MARKER}"))
            .collect();
        let renames = type_params
            .iter()
            .zip(params.iter())
            .map(|(orig, fresh)| (orig.clone(), Type::Named(fresh.clone())))
            .collect();
        Self {
            params,
            originals: type_params.to_vec(),
            renames,
        }
    }

    /// The signature's own name for the fresh parameter `fresh`, or `None`
    /// if `fresh` isn't one of this signature's parameters at all.
    ///
    /// Needed by anything keyed on the *declared* names — bounds, for one,
    /// which are recorded as the signature wrote them while the types being
    /// checked have already been renamed apart.
    pub fn original_of(&self, fresh: &str) -> Option<&String> {
        let i = self.params.iter().position(|p| p == fresh)?;
        self.originals.get(i)
    }

    /// Rewrite a type written in the signature's own parameter names into
    /// fresh space. A non-generic signature renames nothing.
    pub fn rename(&self, ty: &Type) -> Type {
        if self.originals.is_empty() {
            return ty.clone();
        }
        substitute(ty, &self.renames, &self.originals)
    }
}

/// Substitute bound type variables in `ty` with their concrete types from
/// `subst`. Unbound variables (and non-parameter names) are returned as-is.
pub fn substitute(
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
        // `Result<T>` under `T := integer` becomes `Result<integer>`. The
        // *name* is never substituted — a generic's head is a declaration,
        // not a variable — only its arguments.
        Type::Generic(g) => Type::generic(
            g.name.clone(),
            g.args.iter().map(|t| substitute(t, subst, params)).collect(),
        ),
    }
}

/// Whether `ty` still mentions any of `params` after substitution — i.e. a
/// type parameter the call's arguments never pinned down. Such a type is
/// unknown, not concrete, so callers treat it as uninferable rather than
/// letting the bare parameter name escape as if it were a real type.
pub fn mentions_unbound_param(ty: &Type, params: &[String]) -> bool {
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
        Type::Generic(g) => g.args.iter().any(|t| mentions_unbound_param(t, params)),
    }
}

/// The type each parameter slot actually expects at a call site, with the
/// signature's own generics bound from the argument types.
///
/// `filter<T>(items: table<T>, predicate: fn(T) -> boolean)` handed a
/// `table<integer>` expects `fn(integer) -> boolean` in slot 1 — and that
/// is where an untyped lambda's parameters get their types from.
///
/// A slot still mentioning a parameter nothing pinned down comes back
/// `None`: not a type anyone wrote, and refining a lambda against it
/// would put a bare parameter name on the reader's screen.
pub fn instantiate_param_types(
    params: &[Type],
    type_params: &[String],
    arg_types: &[Option<Type>],
) -> Vec<Option<Type>> {
    let fresh = Freshened::new(type_params);
    let renamed: Vec<Type> = params.iter().map(|t| fresh.rename(t)).collect();
    let mut subst = std::collections::HashMap::new();
    // Every argument binds before anything is read back, so a lambda in
    // slot 1 sees what slot 0 pinned down.
    for (expected, found) in renamed.iter().zip(arg_types.iter()) {
        if let Some(found_ty) = found {
            unify(expected, found_ty, &fresh.params, &mut subst);
        }
    }
    renamed
        .iter()
        .map(|t| {
            let resolved = substitute(t, &subst, &fresh.params);
            (!mentions_unbound_param(&resolved, &fresh.params)).then_some(resolved)
        })
        .collect()
}

/// One-way unification: bind type-param names in `expected` to corresponding
/// concrete shapes from `found`. Conservative — if shapes don't line up,
/// silently skip (the regular compatibility check will surface the mismatch).
pub fn unify(
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
        // `Result<T>` against `Result<integer>` binds `T := integer`.
        // The heads must match: two different generics say nothing about
        // each other's arguments, and pairing them positionally would bind
        // `T` from an unrelated declaration.
        (Type::Generic(e), Type::Generic(f))
            if e.name == f.name && e.args.len() == f.args.len() =>
        {
            for (a, b) in e.args.iter().zip(f.args.iter()) {
                unify(a, b, params, subst);
            }
        }
        _ => {}
    }
}

pub fn is_any(t: &Type) -> bool {
    matches!(t, Type::Named(n) if n == "any")
}
