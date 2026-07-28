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

    /// Declared return type of the function, method or lambda whose body is
    /// currently being walked. `None` means "don't check returns here" —
    /// either nothing was declared, or we are inside a lambda that declared
    /// nothing, which must not inherit the enclosing function's type.
    ///
    /// This is a thread-local for the same reason [`CURRENT_CLASS`] is:
    /// `check_stmt` is a plain recursive walk with no context parameter, and
    /// the alternative — a second pass over the body — is what made returns
    /// unsound. That pass re-walked nested blocks with the scope left behind
    /// by the first, which never saw bindings made inside an `if` or a
    /// `while`, so `return found` for a local declared there had no type to
    /// check and was skipped entirely.
    static RETURN_TY: RefCell<Option<Type>> = const { RefCell::new(None) };
}

pub(super) fn current_class() -> Option<String> {
    CURRENT_CLASS.with(|c| c.borrow().clone())
}

pub(super) fn set_current_class(name: Option<String>) -> Option<String> {
    CURRENT_CLASS.with(|c| std::mem::replace(&mut *c.borrow_mut(), name))
}

/// The return type `return v` is checked against right here, if any.
pub(super) fn current_return_ty() -> Option<Type> {
    RETURN_TY.with(|r| r.borrow().clone())
}

/// Install `ty` for the body about to be walked, handing back the previous
/// value. Every caller must restore it — a nested `fn`, method or lambda
/// body sets its own, and letting one leak outward would check a `return`
/// against a signature it has nothing to do with.
pub(super) fn set_return_ty(ty: Option<Type>) -> Option<Type> {
    RETURN_TY.with(|r| std::mem::replace(&mut *r.borrow_mut(), ty))
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
