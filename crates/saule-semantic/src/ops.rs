//! Reading operator overloads off the class registry.
//!
//! When an operand of `+`, `..`, `#`, … is a class instance, the built-in
//! numeric/string rules don't apply: the operator means whatever the class's
//! `Op*` interface method says it means. Answering "what does this evaluate
//! to?" is a matter of finding that method and reading its declared return
//! type — the class registry already holds both halves.
//!
//! # Why here
//!
//! This lived in `saule_typeck::ops`, alongside the *diagnostics* for a
//! missing or ill-typed contract. Those genuinely belong to the checker:
//! they need its scope, its inference and its error type. These do not.
//! They need the class registry and nothing else, and the registry is
//! here — so a reader upstream of `saule-typeck` (this crate's own
//! return-type inference, for one) could not reach the answer at all, and
//! `a + b` on an overloading class was untypeable outside the checker.
//!
//! Note this is a different split from the one that produced `saule-sigs`.
//! The native signature table could move *below* `saule-semantic` because
//! it reads nothing but its own tables; these read the class registry, so
//! below is exactly where they cannot go.

use saule_ast::Type;
use saule_ast::ops::OperatorContract;

use crate::registry::{
    ClassRegistry, InterfaceRegistry, MethodSig, class_implements, lookup_method,
    with_classes,
};

/// Where an overload lookup reads its declarations from.
///
/// The thread-local registries are the right answer for the typechecker,
/// which runs after they are installed. They are the *wrong* answer for a
/// pass that runs while they are still being built — it would consult the
/// previous module's classes, or none at all. Such a caller passes the
/// registry it is holding instead.
#[derive(Clone, Copy)]
pub enum Decls<'a> {
    /// The installed thread-local registries.
    Installed,
    /// Registries under construction, not yet installed.
    Pending {
        classes: &'a ClassRegistry,
        interfaces: &'a InterfaceRegistry,
    },
}

impl Decls<'_> {
    fn is_class(self, name: &str) -> bool {
        match self {
            Decls::Installed => with_classes(|reg| reg.contains_key(name)),
            Decls::Pending { classes, .. } => classes.contains_key(name),
        }
    }

    fn implements(self, class: &str, target: &str) -> bool {
        match self {
            Decls::Installed => class_implements(class, target),
            // Mirrors `class_implements`: walk the parent chain, and count
            // an entry that extends the target as well as one that is it.
            Decls::Pending {
                classes,
                interfaces,
            } => {
                let mut cur = Some(class.to_string());
                while let Some(name) = cur {
                    let Some(info) = classes.get(&name) else {
                        return false;
                    };
                    if info
                        .implements
                        .iter()
                        .any(|i| extends_in(interfaces, i, target))
                    {
                        return true;
                    }
                    cur = info.parent.clone();
                }
                false
            }
        }
    }

    fn method(self, class: &str, method: &str) -> Option<MethodSig> {
        match self {
            Decls::Installed => lookup_method(class, method),
            Decls::Pending { classes, .. } => {
                let mut cur = Some(class.to_string());
                while let Some(name) = cur {
                    let info = classes.get(&name)?;
                    if let Some(sig) = info.methods.get(method) {
                        return Some(sig.clone());
                    }
                    cur = info.parent.clone();
                }
                None
            }
        }
    }
}

/// [`interface_extends`] against a registry that isn't installed yet.
fn extends_in(interfaces: &InterfaceRegistry, iface: &str, target: &str) -> bool {
    if iface == target {
        return true;
    }
    let mut stack = vec![iface.to_string()];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur.clone()) {
            continue;
        }
        if cur == target {
            return true;
        }
        if let Some(parents) = interfaces.get(&cur) {
            stack.extend(parents.iter().cloned());
        }
    }
    false
}

/// The class a *type* denotes, when it denotes one. Operator overloading
/// applies to class instances only — a `table`, an `any`, or a primitive
/// keeps the built-in behaviour.
pub fn class_of(ty: &Type) -> Option<String> {
    class_of_in(Decls::Installed, ty)
}

fn class_of_in(decls: Decls<'_>, ty: &Type) -> Option<String> {
    let bare = match ty {
        Type::Nullable(inner) => (**inner).clone(),
        other => other.clone(),
    };
    let Type::Named(name) = bare else {
        return None;
    };
    decls.is_class(&name).then_some(name)
}

/// Can `class` be the receiver of `contract`'s operator? It must both
/// declare the interface — the opt-in the language asks for — and define
/// the method, which is what dispatch actually calls. Either half may come
/// from a parent class.
pub fn honours(class: &str, contract: &OperatorContract) -> bool {
    class_implements(class, contract.interface) && lookup_method(class, contract.method).is_some()
}

/// Declared return type of `class`'s contract method, e.g. the `Vec2` in
/// `fn add(other: Vec2) -> Vec2`.
///
/// Reading the class's signature rather than the interface's type argument
/// keeps this exact without a generic-instantiation pass: a class declaring
/// `implements OpAdd<Vec2, Vec2>` has to define `fn add(other: Vec2) -> Vec2`
/// anyway, and that declaration is already in the registry.
pub fn result_ty(class: &str, contract: &OperatorContract) -> Option<Type> {
    lookup_method(class, contract.method)?.return_ty
}

/// Declared type of the contract method's single parameter.
pub fn operand_ty(class: &str, contract: &OperatorContract) -> Option<Type> {
    lookup_method(class, contract.method)?
        .params
        .first()
        .map(|p| p.ty.clone())
}

/// Result type of `lhs op rhs` when `lhs` is a class that overloads `op`.
///
/// `None` when the operand is not an overloading class, which leaves the
/// built-in rule for the caller to apply. Comparisons never reach an
/// overload's return type — they are `boolean` however they are
/// implemented — so they answer `None` too.
pub fn overload_binary_result(op: saule_ast::BinOp, lhs_ty: &Type) -> Option<Type> {
    overload_binary_result_in(Decls::Installed, op, lhs_ty)
}

/// [`overload_binary_result`] against a specific set of declarations.
pub fn overload_binary_result_in(
    decls: Decls<'_>,
    op: saule_ast::BinOp,
    lhs_ty: &Type,
) -> Option<Type> {
    use saule_ast::BinOp;
    let contract = saule_ast::ops::binary_contract(op)?;
    if matches!(
        op,
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq
    ) {
        return None;
    }
    let class = class_of_in(decls, lhs_ty)?;
    honours_in(decls, &class, &contract).then(|| result_ty_in(decls, &class, &contract))?
}

/// Result type of `op rhs` when `rhs` is a class that overloads `op`.
pub fn overload_unary_result(op: saule_ast::UnaryOp, rhs_ty: &Type) -> Option<Type> {
    overload_unary_result_in(Decls::Installed, op, rhs_ty)
}

/// [`overload_unary_result`] against a specific set of declarations.
pub fn overload_unary_result_in(
    decls: Decls<'_>,
    op: saule_ast::UnaryOp,
    rhs_ty: &Type,
) -> Option<Type> {
    let contract = saule_ast::ops::unary_contract(op)?;
    let class = class_of_in(decls, rhs_ty)?;
    honours_in(decls, &class, &contract).then(|| result_ty_in(decls, &class, &contract))?
}

fn honours_in(decls: Decls<'_>, class: &str, contract: &OperatorContract) -> bool {
    decls.implements(class, contract.interface) && decls.method(class, contract.method).is_some()
}

fn result_ty_in(decls: Decls<'_>, class: &str, contract: &OperatorContract) -> Option<Type> {
    decls.method(class, contract.method)?.return_ty
}
