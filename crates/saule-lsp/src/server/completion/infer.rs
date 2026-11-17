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

/// What inference needs to know about the position it is answering for: the
/// bindings in scope and the enclosing class. Both the finished [`Found`] and
/// the [`Walk`] still descending to it can answer that — the walk needs it to
/// type a `for` loop's variable from its iterable before the caret is reached.
pub(crate) trait Scope {
    fn bindings(&self) -> &[Visible];
    fn class(&self) -> Option<&String>;

    /// The innermost binding called `name`, so shadowing reads as it does in
    /// the source.
    fn binding(&self, name: &str) -> Option<&Visible> {
        self.bindings().iter().rev().find(|v| v.name == name)
    }
}

impl Scope for Found {
    fn bindings(&self) -> &[Visible] {
        &self.scope
    }
    fn class(&self) -> Option<&String> {
        self.class.as_ref()
    }
}

pub(crate) fn infer(expr: &Expr, found: &impl Scope) -> Option<Recv> {
    infer_d(expr, found, 0)
}

/// Chasing un-annotated bindings (`local a = b`, `local b = Player(…)`) can
/// hop from one initialiser to the next, so cap the depth.
pub(crate) const MAX_INFER_DEPTH: usize = 8;

pub(crate) fn infer_d(expr: &Expr, found: &impl Scope, depth: usize) -> Option<Recv> {
    if depth > MAX_INFER_DEPTH {
        return None;
    }
    match expr {
        // `self` is a keyword with its own node, not an identifier.
        Expr::Self_ => found.class().cloned().map(Recv::SelfClass),
        Expr::Ident(n) => {
            // `self` is also legal as an explicit parameter name.
            if n == "self" {
                return found.class().cloned().map(Recv::SelfClass);
            }
            // A binding in scope shadows any global of the same name.
            if let Some(v) = found.binding(n) {
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
        // `items[1].` / `events[i]?.` — an element of a table is whatever the
        // table holds, so the container's full type has to be known first.
        Expr::Index { .. } | Expr::SafeIndex { .. } => value_type(expr, found, depth + 1)
            .as_ref()
            .and_then(class_of)
            .map(Recv::Instance),
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

/// `T?` → `T`. A nullable container is a bug the checker reports; completion
/// still has to answer for what is inside it.
pub(crate) fn strip_nullable(ty: Type) -> Type {
    match ty {
        Type::Nullable(inner) => *inner,
        other => other,
    }
}

// ─── candidates ─────────────────────────────────────────────────────────────

/// The type a binding's initialiser gives it — for ranking, and for naming
/// the container a `for` loop iterates. Weaker than the checker, like the
/// rest of this file: a literal, a call whose return type is declared, an
/// operator whose result is fixed, a field, an element of a table, or
/// another binding.
///
/// Unlike [`infer_d`] this keeps the *whole* type rather than a class name,
/// which is what lets `Event.poll()` answer `table<Event>` and a loop over
/// it bind `Event`.
pub(crate) fn value_type(expr: &Expr, found: &impl Scope, depth: usize) -> Option<Type> {
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
            let v = found.binding(n)?;
            v.ty.clone()
                .or_else(|| value_type(v.init.as_ref()?, found, depth + 1))
        }
        // `self.items` / `game.world` — the field's declared type, in full.
        Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
            match infer_d(&obj.value, found, depth + 1)? {
                Recv::Module(m) => sigs::lookup_const(&format!("{m}.{name}")),
                Recv::Instance(c) | Recv::Static(c) | Recv::SelfClass(c) => {
                    lookup_field_type(&c, name)
                }
                Recv::Enum(_) => None,
            }
        }
        // `items[i]` — an element of whatever table `items` is.
        Expr::Index { obj, .. } | Expr::SafeIndex { obj, .. } => {
            match strip_nullable(value_type(&obj.value, found, depth + 1)?) {
                Type::Table { value, .. } => Some(*value),
                _ => None,
            }
        }
        Expr::ForceUnwrap(inner) => value_type(&inner.value, found, depth + 1).map(strip_nullable),
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
                && found.binding(m).is_none()
                && let Some(sig) = sigs::lookup(&format!("{m}.{name}"))
            {
                return sig_return(&sig);
            }
            // `world.creatures()` / `Pool.all()` — a method's declared return
            // type, in full, so a method handing back a table can be looped
            // over. Native objects keep theirs in the signature registry.
            if let Expr::Member { obj, name } | Expr::SafeMember { obj, name } = &callee.value
                && let Some(Recv::Instance(c) | Recv::Static(c) | Recv::SelfClass(c)) =
                    infer_d(&obj.value, found, depth + 1)
            {
                return match lookup_method(&c, name) {
                    Some(m) => m.return_ty,
                    None => sig_return(&sigs::lookup(&format!("{c}.{name}"))?),
                };
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
