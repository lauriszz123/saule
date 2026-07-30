//! Static signatures for native (Rust-implemented) functions.
//!
//! The runtime's `NativeFn` / `NativeClosure` values carry only a `name` and
//! a callable — no type information. The typechecker can't inspect a Rust
//! closure, so we maintain a side-table mapping qualified names like
//! `"String.byte"` or bare names like `"assert"` to their declared signatures.
//!
//! Each `stdlib::*::install` function registers the signatures of the natives
//! it defines. The typechecker (`crate::expr`) consults [`lookup`] for
//! `Expr::Call` where the callee resolves to a bare ident or a `Class.method`
//! pair, so writing `local n: integer = String.byte("a")` produces the same
//! `NullableToNonNullable` diagnostic as a user-defined function returning
//! `integer?`.
//!
//! ## Embedder responsibility
//!
//! Because this crate doesn't link the stdlib, embedders must install an
//! initializer via [`set_initializer`]. The hook fires lazily on the first
//! [`lookup`] call (per thread) so the runtime's stdlib registration only
//! runs when actually needed. If no initializer is set, [`lookup`] returns
//! `None` and the typechecker conservatively skips the call site.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use saule_ast::Type;

/// A native function's static signature.
#[derive(Clone, Debug)]
pub struct NativeSig {
    /// Type parameters introduced by this signature (e.g. `["V"]` for
    /// `Table.insert<V>(table<V>, V, integer?)`). Names listed here are
    /// treated as type variables when checking — they're unified against
    /// the *actual* argument types, and the resulting substitution is
    /// applied when checking later positional slots and when inferring
    /// the return type.
    pub type_params: Vec<String>,
    /// Declared positional parameter types.
    pub params: Vec<Type>,
    /// Type of additional trailing arguments. `Some(T)` makes the call
    /// variadic — every extra positional arg must be a `T`. `None` means
    /// exactly `params.len()` positional args are allowed (callers may also
    /// pass fewer if the missing slots are nullable / `any`).
    pub variadic: Option<Type>,
    /// Declared return types. `len() == 1` for single-return functions;
    /// `len() > 1` for multi-return (surfaced as `Type::Tuple` to callers).
    pub returns: Vec<Type>,
}

/// Process-global initializer slot. Set once by the embedder.
static INITIALIZER: OnceLock<fn()> = OnceLock::new();

thread_local! {
    static SIGS: RefCell<HashMap<String, NativeSig>> = RefCell::new(HashMap::new());
    /// `module -> { member names }` — knows every public member of every
    /// stdlib "static class" (`Table`, `String`, `Math`, `Os`, `Io`, …)
    /// regardless of whether a signature is registered. The typechecker
    /// consults this to decide whether `Foo.bar` deserves an "unknown
    /// member" diagnostic (avoiding false positives for value-only fields
    /// like `Math.pi` or `Os.sep`).
    static MEMBERS: RefCell<HashMap<String, std::collections::HashSet<String>>> =
        RefCell::new(HashMap::new());
    /// Names registered via [`register_module`] — stdlib *value types*
    /// (e.g. `File`) whose entries in [`MEMBERS`] describe *instance*
    /// methods, not statically-reachable members. Tracked separately so
    /// the unknown-member check can reject bare `File.write(...)`-style
    /// static access while still accepting `file.write(...)` on an
    /// instance of type `File`.
    static VALUE_TYPES: RefCell<std::collections::HashSet<String>> =
        RefCell::new(std::collections::HashSet::new());
    /// `"Module.member" -> type` for stdlib members that are *values*
    /// rather than callables — `Math.pi`, `Os.sep`, `Io.stdout`. These
    /// have no signature (they aren't functions), but they do have a
    /// type, and without one every annotated use (`local x: float =
    /// Math.pi`) fails with `UndeterminedType`. Kept separate from
    /// [`SIGS`] so a constant can never be mistaken for a zero-arg
    /// function — registering `Math.huge` in `SIGS` is exactly the bug
    /// this table exists to prevent.
    static CONSTS: RefCell<HashMap<String, Type>> = RefCell::new(HashMap::new());
    static INIT_DONE: Cell<bool> = const { Cell::new(false) };
}

/// Register `name -> sig`. Overwrites silently — the embedder's initializer
/// is run at most once per thread. Qualified names (`"Module.member"`) are
/// auto-recorded in the members registry too so `Module.member` is treated
/// as a known field even when the sig is consulted via different code paths.
pub fn register(name: &str, params: Vec<Type>, returns: Vec<Type>) {
    record_member(name);
    SIGS.with(|s| {
        s.borrow_mut().insert(
            name.to_string(),
            NativeSig {
                type_params: Vec::new(),
                params,
                variadic: None,
                returns,
            },
        );
    });
}

/// Register a generic native: `type_params` names are treated as type
/// variables, unified against actual arg types and substituted into both
/// later parameter slots and the return list. Use for
/// `Table.insert<V>(table<V>, V, integer?)`, `Table.remove<V>(table<V>, integer?) -> V?`,
/// etc.
pub fn register_g(
    name: &str,
    type_params: Vec<&str>,
    params: Vec<Type>,
    returns: Vec<Type>,
) {
    record_member(name);
    SIGS.with(|s| {
        s.borrow_mut().insert(
            name.to_string(),
            NativeSig {
                type_params: type_params.into_iter().map(String::from).collect(),
                params,
                variadic: None,
                returns,
            },
        );
    });
}

/// Register a variadic native: any extra trailing positional args must match
/// `variadic`. Use for `printf(fmt, ...)`, `String.char(...integer)`, etc.
pub fn register_v(name: &str, params: Vec<Type>, variadic: Type, returns: Vec<Type>) {
    record_member(name);
    SIGS.with(|s| {
        s.borrow_mut().insert(
            name.to_string(),
            NativeSig {
                type_params: Vec::new(),
                params,
                variadic: Some(variadic),
                returns,
            },
        );
    });
}

/// Record `Module.member` as a known stdlib member without attaching a
/// callable signature. Used for natives whose signature is intentionally
/// left unmodelled (e.g. `Math.abs`, `Math.min`, where the return type
/// depends on input flavour). Bare names (no `.`) are ignored.
///
/// For members that hold a *value* of a known type, prefer
/// [`register_const`] — this function records only the name, so an
/// annotated use still fails with `UndeterminedType`.
pub fn register_member(qname: &str) {
    record_member(qname);
}

/// Register `Module.member` as a typed **constant** — a member that holds
/// a value rather than a callable (`Math.pi`, `Os.sep`, `Io.stdout`).
///
/// This is the counterpart to [`register`]: use that for things you call,
/// this for things you read. Registering a constant through `register`
/// with an empty parameter list makes the checker believe it's a zero-arg
/// function, which then fails both ways — bare use can't be typed, and the
/// call the signature invites blows up at runtime on a non-callable value.
pub fn register_const(qname: &str, ty: Type) {
    record_member(qname);
    CONSTS.with(|c| {
        c.borrow_mut().insert(qname.to_string(), ty);
    });
}

/// Mark `name` as a known stdlib *type* / module that has **no** static
/// members. Used for value classes like `File` whose methods are only
/// reachable through an instance (`f:write(...)`) — recording the bare
/// module makes [`is_module`] return `true` so static-call typos like
/// `File.write(...)` get flagged by the unknown-member check instead of
/// silently passing typeck and exploding at runtime.
pub fn register_module(name: &str) {
    MEMBERS.with(|m| {
        m.borrow_mut().entry(name.to_string()).or_default();
    });
    VALUE_TYPES.with(|v| {
        v.borrow_mut().insert(name.to_string());
    });
}

fn record_member(qname: &str) {
    let Some((module, member)) = qname.split_once('.') else {
        return;
    };
    MEMBERS.with(|m| {
        m.borrow_mut()
            .entry(module.to_string())
            .or_default()
            .insert(member.to_string());
    });
}

/// Install the initializer the embedder wants to run lazily on first
/// [`lookup`]. Typically called once at startup by the interpreter so its
/// stdlib signatures appear in the registry before any type-check pass.
///
/// Subsequent calls are silently ignored (`OnceLock` semantics).
pub fn set_initializer(f: fn()) {
    let _ = INITIALIZER.set(f);
}

/// Look up by qualified (`"String.byte"`) or bare (`"assert"`) name.
///
/// Returns `None` when the name isn't registered — the typechecker treats
/// that as "unknown call" and skips signature-based checks rather than
/// emitting a false positive.
pub fn lookup(name: &str) -> Option<NativeSig> {
    ensure_registered();
    SIGS.with(|s| s.borrow().get(name).cloned())
}

/// Look up the declared type of a stdlib constant by qualified name
/// (`"Math.pi"`). Returns `None` for callables and unknown names.
pub fn lookup_const(qname: &str) -> Option<Type> {
    ensure_registered();
    CONSTS.with(|c| c.borrow().get(qname).cloned())
}

/// Bind a generic native sig's type parameters from the inferred types of
/// its positional arguments (in declaration order; `None` where inference
/// failed) and return the substituted return list. Non-generic sigs clone
/// `sig.returns` unchanged.
///
/// Lets tools like the LSP surface the concrete inferred type
/// (`Util.filter(table<integer>) -> table<integer>`) instead of the raw
/// type parameter (`table<T>`).
pub fn instantiate_returns(sig: &NativeSig, arg_types: &[Option<Type>]) -> Vec<Type> {
    if sig.type_params.is_empty() {
        return sig.returns.clone();
    }
    let mut subst = HashMap::new();
    for (expected, found) in sig.params.iter().zip(arg_types.iter()) {
        if let Some(found_ty) = found {
            crate::expr::unify(expected, found_ty, &sig.type_params, &mut subst);
        }
    }
    sig.returns
        .iter()
        .map(|t| crate::expr::substitute(t, &subst, &sig.type_params))
        .collect()
}

/// Does `ty` still mention one of `type_params` after instantiation?
///
/// [`instantiate_returns`] leaves a type parameter the arguments never
/// pinned down standing as its own bare name, so `map(t, f)` can come
/// back as `table<U>`. That isn't a type the user wrote or can act on —
/// tools should treat it as "not inferred" rather than render it. Thin
/// re-export of the checker's own post-substitution guard so the LSP
/// applies exactly the same rule.
pub fn mentions_unbound_param(ty: &Type, type_params: &[String]) -> bool {
    crate::expr::mentions_unbound_param(ty, type_params)
}

/// Same as [`instantiate_returns`] but for a *semantic* method signature
/// (e.g. a dynamic native package's class method seeded into
/// `saule_semantic`). Returns the substituted return type, or `None` when
/// the method has no declared return.
pub fn instantiate_method_return(
    sig: &saule_semantic::MethodSig,
    arg_types: &[Option<Type>],
) -> Option<Type> {
    let ret = sig.return_ty.clone()?;
    if sig.type_params.is_empty() {
        return Some(ret);
    }
    let mut subst = HashMap::new();
    for (p, found) in sig.params.iter().zip(arg_types.iter()) {
        if let Some(found_ty) = found {
            crate::expr::unify(&p.ty, found_ty, &sig.type_params, &mut subst);
        }
    }
    Some(crate::expr::substitute(&ret, &subst, &sig.type_params))
}

/// Returns `true` when `name` is a known stdlib *module* (i.e. at least
/// one member has been recorded for it). Used by the typechecker to
/// decide whether `Foo.bar` deserves an "unknown member" diagnostic.
pub fn is_module(name: &str) -> bool {
    ensure_registered();
    MEMBERS.with(|m| m.borrow().contains_key(name))
}

/// Is `member` a known public member of stdlib `module`?
pub fn has_member(module: &str, member: &str) -> bool {
    ensure_registered();
    MEMBERS.with(|m| {
        m.borrow()
            .get(module)
            .is_some_and(|set| set.contains(member))
    })
}

/// Was `name` registered via [`register_module`]? Such names refer to
/// stdlib *value types* (instance method holders) rather than true
/// modules — bare `Name.member` static access on them is never valid
/// regardless of the contents of [`MEMBERS`].
pub fn is_value_type(name: &str) -> bool {
    ensure_registered();
    VALUE_TYPES.with(|v| v.borrow().contains(name))
}

/// Collect every recorded member of `module`. Used to power
/// "did-you-mean" hints on unknown-member diagnostics.
pub fn module_members(module: &str) -> Vec<String> {
    ensure_registered();
    MEMBERS.with(|m| {
        m.borrow()
            .get(module)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    })
}

fn ensure_registered() {
    if INIT_DONE.with(Cell::get) {
        return;
    }
    INIT_DONE.with(|d| d.set(true));
    if let Some(f) = INITIALIZER.get() {
        f();
    }
}

// ─── Type-builder shorthands for callers ────────────────────────────────────

pub fn t_named(s: &str) -> Type {
    Type::Named(s.to_string())
}

pub fn t_any() -> Type {
    Type::Named("any".to_string())
}

/// Sentinel meaning "either `integer` or `float`". Recognised by
/// `crate::expr::types_compatible`. Use for math/numeric natives.
pub fn t_number() -> Type {
    Type::Named("number".to_string())
}

pub fn t_nullable(inner: Type) -> Type {
    Type::Nullable(Box::new(inner))
}

pub fn t_table(value: Type) -> Type {
    Type::Table {
        key: None,
        value: Box::new(value),
    }
}

pub fn t_table_map(key: Type, value: Type) -> Type {
    Type::Table {
        key: Some(Box::new(key)),
        value: Box::new(value),
    }
}

pub fn t_function(params: Vec<Type>, ret: Type) -> Type {
    Type::Function {
        params,
        ret: Box::new(ret),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use saule_ast::Param;

    fn param(name: &str, ty: Type) -> Param {
        Param {
            name: name.into(),
            ty,
            default: None,
            variadic: false,
            span: 0..0,
        }
    }

    #[test]
    fn instantiate_returns_binds_table_element() {
        // fn<T>(t: table<T>, f: function) -> table<T>
        let sig = NativeSig {
            type_params: vec!["T".into()],
            params: vec![t_table(t_named("T")), t_named("function")],
            variadic: None,
            returns: vec![t_table(t_named("T"))],
        };
        let args = [Some(t_table(t_named("integer"))), Some(t_named("function"))];
        assert_eq!(
            instantiate_returns(&sig, &args),
            vec![t_table(t_named("integer"))]
        );
    }

    #[test]
    fn instantiate_returns_binds_accumulator_param() {
        // fn<T, U>(t: table<T>, f: function, init: U) -> U
        let sig = NativeSig {
            type_params: vec!["T".into(), "U".into()],
            params: vec![t_table(t_named("T")), t_named("function"), t_named("U")],
            variadic: None,
            returns: vec![t_named("U")],
        };
        let args = [
            Some(t_table(t_named("integer"))),
            Some(t_named("function")),
            Some(t_named("integer")),
        ];
        assert_eq!(instantiate_returns(&sig, &args), vec![t_named("integer")]);
    }

    #[test]
    fn instantiate_returns_non_generic_clones_unchanged() {
        let sig = NativeSig {
            type_params: vec![],
            params: vec![t_named("integer")],
            variadic: None,
            returns: vec![t_named("boolean")],
        };
        assert_eq!(
            instantiate_returns(&sig, &[Some(t_named("integer"))]),
            vec![t_named("boolean")]
        );
    }

    #[test]
    fn instantiate_method_return_substitutes_nullable() {
        // find<T>(t: table<T>, f: function) -> T?
        let sig = saule_semantic::MethodSig {
            is_static: true,
            is_private: false,
            type_params: vec!["T".into()],
            params: vec![
                param("t", t_table(t_named("T"))),
                param("f", t_named("function")),
            ],
            return_ty: Some(t_nullable(t_named("T"))),
        };
        let args = [Some(t_table(t_named("integer"))), Some(t_named("function"))];
        assert_eq!(
            instantiate_method_return(&sig, &args),
            Some(t_nullable(t_named("integer")))
        );
    }
}

