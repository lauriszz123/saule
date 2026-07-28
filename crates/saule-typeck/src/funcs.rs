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

/// Derive the arity facts a [`FunctionInfo`] carries from a parameter list.
/// Defaults must come last in the grammar, so the counts are enough to
/// decide whether a given call site supplies a valid number of arguments.
fn info_from_params(
    params: &[Param],
    type_params: &[String],
    return_ty: &Option<Type>,
) -> FunctionInfo {
    FunctionInfo {
        total: params.len(),
        defaults: params.iter().filter(|p| p.default.is_some()).count(),
        variadic: params.last().is_some_and(|p| p.variadic),
        params: params.to_vec(),
        type_params: type_params.to_vec(),
        return_ty: return_ty.clone(),
    }
}

pub(super) fn install(module: &Module) {
    // Functions this module *imported*. The semantic pass installs them
    // from the embedder's import seed; without them a call to an imported
    // `fn` has no signature at all, so its result type is unknown and any
    // checked position it feeds fails. Locally-declared names overwrite
    // these below — a module's own `fn` always wins.
    let mut map: HashMap<String, FunctionInfo> = saule_semantic::with_functions(|reg| {
        reg.iter()
            .map(|(name, sig)| {
                (
                    name.clone(),
                    info_from_params(&sig.params, &sig.type_params, &sig.return_ty),
                )
            })
            .collect()
    });
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
            map.insert(
                name.clone(),
                info_from_params(params, type_params, return_ty),
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
