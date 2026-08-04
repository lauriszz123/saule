//! The narrow type inference completion needs to answer "what is the
//! receiver of this `.`?" — depth-limited, and deliberately weaker
//! than the checker's.

use saule_ast::{Expr, Type};
use saule_semantic::registry::{lookup_field_type, lookup_method, with_classes, with_enums};
use saule_typeck::sigs::{self, NativeSig};

use super::*;

/// What a `receiver.` expression turned out to be.
pub(crate) enum Recv {
    /// An instance — offer public instance members.
    Instance(String),
    /// A class used by name — offer public statics.
    Static(String),
    /// `self` — offer everything, including private members.
    SelfClass(String),
    /// A stdlib module or native-package class (`Math`, `Window`, …).
    Module(String),
    /// An enum type — offer its variants.
    Enum(String),
}

pub(crate) fn infer(expr: &Expr, found: &Found) -> Option<Recv> {
    infer_d(expr, found, 0)
}

/// Chasing un-annotated bindings (`local a = b`, `local b = Player(…)`) can
/// hop from one initialiser to the next, so cap the depth.
pub(crate) const MAX_INFER_DEPTH: usize = 8;

pub(crate) fn infer_d(expr: &Expr, found: &Found, depth: usize) -> Option<Recv> {
    if depth > MAX_INFER_DEPTH {
        return None;
    }
    match expr {
        // `self` is a keyword with its own node, not an identifier.
        Expr::Self_ => found.class.clone().map(Recv::SelfClass),
        Expr::Ident(n) => {
            // `self` is also legal as an explicit parameter name.
            if n == "self" {
                return found.class.clone().map(Recv::SelfClass);
            }
            // A binding in scope shadows any global of the same name.
            if let Some(v) = found.scope.iter().rev().find(|v| &v.name == n) {
                if let Some(c) = v.ty.as_ref().and_then(class_of) {
                    return Some(Recv::Instance(c));
                }
                // Un-annotated: infer from what it was initialised with.
                return v.init.as_ref().and_then(|e| infer_d(e, found, depth + 1));
            }
            if with_classes(|r| r.contains_key(n)) {
                return Some(Recv::Static(n.clone()));
            }
            if with_enums(|r| r.contains_key(n)) {
                return Some(Recv::Enum(n.clone()));
            }
            if sigs::is_module(n) {
                return Some(Recv::Module(n.clone()));
            }
            None
        }
        // `a.b.` / `self.player.` — the field's declared type carries on.
        Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
            let owner = owner_class_d(&obj.value, found, depth + 1)?;
            lookup_field_type(&owner, name)
                .as_ref()
                .and_then(class_of)
                .map(Recv::Instance)
        }
        // `Player("x").` / `make().` — the callee's return type.
        Expr::Call { callee, .. } => match &callee.value {
            Expr::Ident(n) if with_classes(|r| r.contains_key(n)) => {
                Some(Recv::Instance(n.clone()))
            }
            Expr::Ident(n) => sig_return(&sigs::lookup(n)?)
                .and_then(named)
                .map(Recv::Instance),
            Expr::Member { obj, name } => match infer_d(&obj.value, found, depth + 1)? {
                Recv::Module(m) => sig_return(&sigs::lookup(&format!("{m}.{name}"))?)
                    .and_then(named)
                    .map(Recv::Instance),
                Recv::Instance(c) | Recv::Static(c) | Recv::SelfClass(c) => {
                    lookup_method(&c, name)?
                        .return_ty
                        .as_ref()
                        .and_then(class_of)
                        .map(Recv::Instance)
                }
                Recv::Enum(_) => None,
            },
            _ => None,
        },
        // `maybe!.` — force-unwrap keeps the underlying type.
        Expr::ForceUnwrap(inner) => infer_d(&inner.value, found, depth + 1),
        _ => None,
    }
}

/// The class that owns members reached through `expr`.
pub(crate) fn owner_class_d(expr: &Expr, found: &Found, depth: usize) -> Option<String> {
    match infer_d(expr, found, depth)? {
        Recv::Instance(c) | Recv::Static(c) | Recv::SelfClass(c) => Some(c),
        _ => None,
    }
}

/// A type's class name, seeing through `T?`.
pub(crate) fn class_of(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => class_of(inner),
        _ => None,
    }
}

pub(crate) fn sig_return(sig: &NativeSig) -> Option<Type> {
    sig.returns.first().cloned()
}

pub(crate) fn named(ty: Type) -> Option<String> {
    class_of(&ty)
}

// ─── candidates ─────────────────────────────────────────────────────────────
