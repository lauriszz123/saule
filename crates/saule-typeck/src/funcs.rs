//! Lookup of top-level user-defined functions, indexed by name. Built at
//! the start of [`crate::check`] and consulted in `expr.rs` to give
//! `add(1)` a clean `FunctionArity` diagnostic when `add` declares two
//! required parameters.
//!
//! Class methods and lambdas aren't tracked here; arity for those still
//! falls through to the runtime. Static method arity could be added under
//! `ClassName.method` keys in a follow-up.

use std::cell::RefCell;
use std::collections::HashMap;

use saule_ast::{Decl, Module, Param, Stmt, Type};

#[derive(Clone, Debug)]
pub(super) struct FunctionInfo {
    /// Total number of declared parameters (including any with defaults).
    pub(super) total: usize,
    /// How many of the declared parameters have a default value. Defaults
    /// must appear last in Saule's grammar, so a call is valid when
    /// `found ∈ [total - defaults, total]` (or any number when variadic).
    pub(super) defaults: usize,
    /// Whether the last parameter is variadic — when true, calls with
    /// `found >= total - 1 - defaults` are all valid.
    pub(super) variadic: bool,
    /// Full parameter list, kept so the typechecker can validate
    /// `when(x):name(args)` stage calls (where the receiver type is
    /// matched against `params[0].ty`) and other per-arg type rules.
    pub(super) params: Vec<Param>,
    /// Generic type parameters declared with `<T, U>` after the function
    /// name. Used by the caller-side argument checker to unify across
    /// `T`-typed slots.
    pub(super) type_params: Vec<String>,
    /// Declared return type, if any. `None` is treated as "unknown" and
    /// causes the typechecker to skip downstream inference.
    pub(super) return_ty: Option<Type>,
}

thread_local! {
    static FUNCTIONS: RefCell<HashMap<String, FunctionInfo>> =
        RefCell::new(HashMap::new());
}

pub(super) fn install(module: &Module) {
    let mut map: HashMap<String, FunctionInfo> = HashMap::new();
    for stmt in &module.stmts {
        if let Stmt::Decl(d) = &stmt.value
            && let Decl::Function {
                name,
                params,
                return_ty,
                type_params,
                ..
            } = &d.value
        {
            let total = params.len();
            let defaults = params.iter().filter(|p| p.default.is_some()).count();
            let variadic = params.last().is_some_and(|p| p.variadic);
            map.insert(
                name.clone(),
                FunctionInfo {
                    total,
                    defaults,
                    variadic,
                    params: params.clone(),
                    type_params: type_params.clone(),
                    return_ty: return_ty.clone(),
                },
            );
        }
    }
    FUNCTIONS.with(|f| *f.borrow_mut() = map);
}

pub(super) fn clear() {
    FUNCTIONS.with(|f| f.borrow_mut().clear());
}

pub(super) fn lookup(name: &str) -> Option<FunctionInfo> {
    FUNCTIONS.with(|f| f.borrow().get(name).cloned())
}
