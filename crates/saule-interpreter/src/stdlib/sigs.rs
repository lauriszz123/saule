//! Static signatures for native (Rust-implemented) functions.
//!
//! The runtime's `NativeFn` / `NativeClosure` values carry only a `name` and
//! a callable — no type information. The typechecker can't inspect a Rust
//! closure, so we maintain a side-table mapping qualified names like
//! `"String.byte"` or bare names like `"assert"` to their declared signatures.
//!
//! Each `stdlib::*::install` function registers the signatures of the natives
//! it defines. The typechecker (`typeck::infer`) consults `lookup` for
//! `Expr::Call` where the callee resolves to a bare ident or a `Class.method`
//! pair, so writing `local n: integer = String.byte("a")` produces the same
//! `NullableToNonNullable` diagnostic as a user-defined function returning
//! `integer?`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Once;

use saule_ast::Type;

/// A native function's static signature.
#[derive(Clone, Debug)]
pub struct NativeSig {
    /// Declared parameter types (currently unused by the checker; reserved).
    pub params: Vec<Type>,
    /// Declared return types. `len() == 1` for single-return functions;
    /// `len() > 1` for multi-return (surfaced as `Type::Tuple` to callers).
    pub returns: Vec<Type>,
}

thread_local! {
    static SIGS: RefCell<HashMap<String, NativeSig>> = RefCell::new(HashMap::new());
}

/// Register `name -> sig`. Overwrites silently — `install_std` is idempotent
/// per process.
pub fn register(name: &str, params: Vec<Type>, returns: Vec<Type>) {
    SIGS.with(|s| {
        s.borrow_mut().insert(
            name.to_string(),
            NativeSig { params, returns },
        );
    });
}

/// Look up by qualified (`"String.byte"`) or bare (`"assert"`) name.
///
/// The typechecker calls this *before* `install_std` runs in the CLI (since
/// type-checking happens prior to environment construction). To make sigs
/// available regardless of call order, the first lookup triggers a one-shot
/// registration of every stdlib module's signatures.
pub fn lookup(name: &str) -> Option<NativeSig> {
    ensure_registered();
    SIGS.with(|s| s.borrow().get(name).cloned())
}

static REGISTER_ONCE: Once = Once::new();

fn ensure_registered() {
    REGISTER_ONCE.call_once(|| {
        crate::stdlib::core::register_sigs();
        crate::stdlib::math::register_sigs();
        crate::stdlib::string::register_sigs();
        crate::stdlib::iter::register_sigs();
        crate::stdlib::table::register_sigs();
        crate::stdlib::io::register_sigs();
    });
}

// ─── Type-builder shorthands for callers ────────────────────────────────────

pub fn t_named(s: &str) -> Type {
    Type::Named(s.to_string())
}

pub fn t_nullable(inner: Type) -> Type {
    Type::Nullable(Box::new(inner))
}

pub fn t_table(value: Type) -> Type {
    Type::Table { key: None, value: Box::new(value) }
}

pub fn t_table_map(key: Type, value: Type) -> Type {
    Type::Table { key: Some(Box::new(key)), value: Box::new(value) }
}

pub fn t_function(params: Vec<Type>, ret: Type) -> Type {
    Type::Function { params, ret: Box::new(ret) }
}

