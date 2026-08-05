//! Types of module-level variables (`export name: T = value`), indexed by
//! name. Built at the start of [`crate::check`] and consulted from `infer`
//! when a bare identifier misses in the lexical scope.
//!
//! The indirection exists because function and method bodies are checked
//! with a fresh [`Scope`](crate::state::Scope): nothing carries the
//! module's own bindings into them. A module variable is visible
//! file-wide, so it needs a home outside any one scope — the same
//! arrangement [`crate::funcs`] uses for top-level `fn` signatures.
//!
//! Variables imported from other modules land here too, seeded by the
//! embedder through `saule_semantic`'s variable registry, so
//! `import * from "config"` gives `appName` a type at the use site.

use std::cell::RefCell;
use std::collections::HashMap;

use saule_ast::{Decl, Module, Stmt, Type};

thread_local! {
    static VARIABLES: RefCell<HashMap<String, Type>> = RefCell::new(HashMap::new());
}

pub(super) fn install(module: &Module) {
    let mut map: HashMap<String, Type> = saule_semantic::with_variables(|reg| reg.clone());
    for stmt in &module.stmts {
        if let Stmt::Decl(d) = &stmt.value
            && let Decl::Variable { name, ty, .. } = &d.value
            // An un-annotated variable has no declared type to record;
            // `infer` then falls through and treats uses as unknown,
            // exactly as it does for an un-annotated `local`.
            && let Some(t) = ty
        {
            map.insert(name.clone(), t.clone());
        }
    }
    VARIABLES.with(|v| *v.borrow_mut() = map);
}

pub(super) fn clear() {
    VARIABLES.with(|v| v.borrow_mut().clear());
}

pub(crate) fn lookup(name: &str) -> Option<Type> {
    VARIABLES.with(|v| v.borrow().get(name).cloned())
}
