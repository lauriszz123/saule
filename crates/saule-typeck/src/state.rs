//! Typecheck-local state:
//!
//!   * [`Scope`] — per-block static-type environment for `local` bindings.
//!   * Thread-locals tracking the class currently being walked (for
//!     visibility checks) and the generic type-parameter names in scope.
//!
//! The class / interface / enum registries themselves live in
//! `saule-semantic`; this module re-exports the read accessors so call
//! sites inside the typechecker don't need to know which crate physically
//! owns them.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use saule_ast::Type;

pub(super) use saule_semantic::{
    class_implements, class_implements_iterable, is_interface, is_subtype_named, lookup_member,
    with_classes, with_enums,
};

/// Tracks the static types of `local` bindings in lexical scope.
#[derive(Default, Clone)]
pub(super) struct Scope {
    vars: HashMap<String, Type>,
}

impl Scope {
    pub(super) fn lookup(&self, name: &str) -> Option<&Type> {
        self.vars.get(name)
    }

    pub(super) fn bind(&mut self, name: String, ty: Type) {
        self.vars.insert(name, ty);
    }
}

thread_local! {
    static CURRENT_CLASS: RefCell<Option<String>> = const { RefCell::new(None) };

    /// Generic type-parameter names in scope for the function/method body
    /// currently being checked. Treated as `any`-equivalent so that
    /// `table<T>`, `T?`, and bare `T` accept any concrete instantiation.
    static GENERICS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

pub(super) fn current_class() -> Option<String> {
    CURRENT_CLASS.with(|c| c.borrow().clone())
}

pub(super) fn set_current_class(name: Option<String>) -> Option<String> {
    CURRENT_CLASS.with(|c| std::mem::replace(&mut *c.borrow_mut(), name))
}

/// Add `params` to the in-scope generic set. Returns the names actually
/// inserted so the matching [`pop_generics`] can remove just those (and
/// preserve any outer generics that share a name).
pub(super) fn push_generics(params: &[String]) -> Vec<String> {
    let mut added = Vec::new();
    GENERICS.with(|g| {
        let mut set = g.borrow_mut();
        for p in params {
            if set.insert(p.clone()) {
                added.push(p.clone());
            }
        }
    });
    added
}

pub(super) fn pop_generics(added: Vec<String>) {
    GENERICS.with(|g| {
        let mut set = g.borrow_mut();
        for p in added {
            set.remove(&p);
        }
    });
}

/// True if `name` names a type parameter in scope for the current body.
pub(super) fn is_type_param(name: &str) -> bool {
    GENERICS.with(|g| g.borrow().contains(name))
}


