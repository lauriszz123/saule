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
            match infer_d(&obj.value, found, depth + 1)? {
                // `Io.stdout.` — a module *constant* carries its own type.
                Recv::Module(m) => sigs::lookup_const(&format!("{m}.{name}"))
                    .and_then(named)
                    .map(Recv::Instance),
                Recv::Instance(c) | Recv::Static(c) | Recv::SelfClass(c) => {
                    lookup_field_type(&c, name)
                        .as_ref()
                        .and_then(class_of)
                        .map(Recv::Instance)
                }
                Recv::Enum(_) => None,
            }
        }
        // `Player("x").` / `make().` — the callee's return type.
        Expr::Call { callee, .. } => match &callee.value {
            Expr::Ident(n) if with_classes(|r| r.contains_key(n)) => {
                Some(Recv::Instance(n.clone()))
            }
            Expr::Ident(n) => sig_return(&sigs::lookup(n)?)
                .and_then(named)
                .map(Recv::Instance),
            // `recv.method().` and `recv?.method().` chase the same
            // return type — a safe call only adds nullability, and
            // `class_of` looks straight through that.
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                match infer_d(&obj.value, found, depth + 1)? {
                    Recv::Module(m) => sig_return(&sigs::lookup(&format!("{m}.{name}"))?)
                        .and_then(named)
                        .map(Recv::Instance),
                    Recv::Instance(c) | Recv::Static(c) | Recv::SelfClass(c) => {
                        match lookup_method(&c, name) {
                            Some(m) => m.return_ty.as_ref().and_then(class_of).map(Recv::Instance),
                            // A stdlib value type keeps its instance
                            // methods in the native registry instead.
                            None => sig_return(&sigs::lookup(&format!("{c}.{name}"))?)
                                .and_then(named)
                                .map(Recv::Instance),
                        }
                    }
                    Recv::Enum(_) => None,
                }
            }
            _ => None,
        },
        // `maybe!.` — force-unwrap keeps the underlying type.
        Expr::ForceUnwrap(inner) => infer_d(&inner.value, found, depth + 1),
        _ => None,
    }
}

/// A type's class name, seeing through `T?`.
pub(crate) fn class_of(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        Type::Nullable(inner) => class_of(inner),
        // A `Box<integer>` is an instance of `Box`.
        Type::Generic(g) => Some(g.name.clone()),
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

/// The type a binding's initialiser gives it, for ranking. Weaker than the
/// checker, like the rest of this file: a literal, a call whose return type is
/// declared, an operator whose result is fixed, or another binding.
pub(crate) fn value_type(expr: &Expr, found: &Found, depth: usize) -> Option<Type> {
    use saule_ast::{BinOp, UnaryOp};
    if depth > MAX_INFER_DEPTH {
        return None;
    }
    let named = |n: &str| Some(Type::Named(n.into()));
    match expr {
        Expr::Int(_) => named("integer"),
        Expr::Float(_) => named("float"),
        Expr::Str(_) => named("string"),
        Expr::Bool(_) => named("boolean"),
        Expr::Table(_) => named("table"),
        Expr::Ident(n) => {
            let v = found.scope.iter().rev().find(|v| &v.name == n)?;
            v.ty.clone()
                .or_else(|| value_type(v.init.as_ref()?, found, depth + 1))
        }
        Expr::Unary {
            op: UnaryOp::Not, ..
        } => named("boolean"),
        Expr::Unary {
            op: UnaryOp::Len, ..
        } => named("integer"),
        Expr::Binary { op, lhs, .. } => match op {
            BinOp::Concat => named("string"),
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                named("boolean")
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Mod => {
                value_type(&lhs.value, found, depth + 1)
            }
            _ => None,
        },
        Expr::Call { callee, .. } => {
            if let Expr::Ident(n) = &callee.value
                && let Some(sig) = saule_semantic::lookup_function(n)
            {
                return sig.return_ty;
            }
            if let Expr::Ident(n) = &callee.value
                && let Some(sig) = sigs::lookup(n)
            {
                return sig_return(&sig);
            }
            if let Expr::Member { obj, name } = &callee.value
                && let Expr::Ident(m) = &obj.value
                && let Some(sig) = sigs::lookup(&format!("{m}.{name}"))
            {
                return sig_return(&sig);
            }
            match infer_d(expr, found, depth + 1)? {
                Recv::Instance(c) => Some(Type::Named(c)),
                _ => None,
            }
        }
        _ => match infer_d(expr, found, depth + 1)? {
            Recv::Instance(c) => Some(Type::Named(c)),
            _ => None,
        },
    }
}
