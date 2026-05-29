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
    static INIT_DONE: Cell<bool> = const { Cell::new(false) };
}

/// Register `name -> sig`. Overwrites silently — the embedder's initializer
/// is run at most once per thread.
pub fn register(name: &str, params: Vec<Type>, returns: Vec<Type>) {
    SIGS.with(|s| {
        s.borrow_mut().insert(
            name.to_string(),
            NativeSig {
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
    SIGS.with(|s| {
        s.borrow_mut().insert(
            name.to_string(),
            NativeSig {
                params,
                variadic: Some(variadic),
                returns,
            },
        );
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
