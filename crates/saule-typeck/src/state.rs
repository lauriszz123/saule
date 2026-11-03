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

use saule_ast::{Type, TypeBound, TypeRef};

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
    /// currently being checked.
    ///
    /// These are **rigid**: universally quantified by the signature, so
    /// inside the body a `T` is opaque. It is not a supertype of any
    /// concrete type and no concrete type is a `T` — the caller picks
    /// what it stands for, and the body must work for every choice.
    /// Name → the bound it was declared with, if any. A parameter with a
    /// bound is still rigid; the bound only says what members it is known
    /// to have, which is what lets a body call a method on one at all.
    static GENERICS: RefCell<HashMap<String, Option<TypeRef>>> = RefCell::new(HashMap::new());

    /// Type-parameter names belonging to a *signature being called into*,
    /// pushed for the duration of one argument comparison.
    ///
    /// The opposite of [`GENERICS`]: these are inference variables, not
    /// rigid types. One that the arguments haven't pinned down yet binds
    /// to whatever this position supplies, so it stays permissive —
    /// `Table.insert<V>(table<V>, V)` called with an `any` must not be
    /// read as "expects `V`, got `any`".
    static SIG_PARAMS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());

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

/// Add `params` to the in-scope generic set, each with the bound it was
/// declared with. Returns the names actually inserted so the matching
/// [`pop_generics`] can remove just those (and preserve any outer generics
/// that share a name).
pub(super) fn push_generics(params: &[String], bounds: &[TypeBound]) -> Vec<String> {
    let mut added = Vec::new();
    GENERICS.with(|g| {
        let mut set = g.borrow_mut();
        for p in params {
            // Looked up by name: `type_bounds` holds an entry only for the
            // parameters that were written with one.
            let bound = bounds
                .iter()
                .find(|b| &b.param == p)
                .map(|b| b.bound.clone());
            if !set.contains_key(p) {
                set.insert(p.clone(), bound);
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
    GENERICS.with(|g| g.borrow().contains_key(name))
}

/// Resolve a type head for member lookup, following a type parameter's
/// bound.
///
/// `<T: Named>` makes a `T`'s members exactly `Named`'s, so every member
/// lookup on a `T` is really a lookup on its bound — with the bound's own
/// type arguments, so `<T: Box<integer>>` reads `get()` as `integer`.
/// Anything that is not a bounded parameter passes through untouched.
pub(crate) fn through_bound(name: String, args: Vec<Type>) -> (String, Vec<Type>) {
    match type_param_bound(&name) {
        Some(b) => (b.name, b.args),
        None => (name, args),
    }
}

/// The bound `name` was declared with, if it is an in-scope type parameter
/// and was written `<T: Bound>`.
///
/// This is what a bounded parameter is *for*: inside the body `T` has
/// exactly the members its bound declares, so every member lookup on a `T`
/// is answered by looking at the bound instead. An unbounded parameter
/// answers `None`, and a lookup on one is an error — nothing is known about
/// it, so nothing can be called on it.
pub(crate) fn type_param_bound(name: &str) -> Option<TypeRef> {
    GENERICS.with(|g| g.borrow().get(name).cloned().flatten())
}

/// Scope the callee's type-parameter names in for one comparison. Kept
/// separate from [`push_generics`] because the two mean opposite things
/// — see [`SIG_PARAMS`].
pub(super) fn push_sig_params(params: &[String]) -> Vec<String> {
    let mut added = Vec::new();
    SIG_PARAMS.with(|g| {
        let mut set = g.borrow_mut();
        for p in params {
            if set.insert(p.clone()) {
                added.push(p.clone());
            }
        }
    });
    added
}

pub(super) fn pop_sig_params(added: Vec<String>) {
    SIG_PARAMS.with(|g| {
        let mut set = g.borrow_mut();
        for p in added {
            set.remove(&p);
        }
    });
}

/// True if `name` is an inference variable of the signature currently
/// being called into.
pub(super) fn is_sig_param(name: &str) -> bool {
    SIG_PARAMS.with(|g| g.borrow().contains(name))
}
