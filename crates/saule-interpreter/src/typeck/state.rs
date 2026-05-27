//! Shared state for the typechecker:
//!
//!   * [`Scope`] — per-block static-type environment for `local` bindings.
//!   * Thread-local registries of classes, interfaces, and enums (populated
//!     by a pre-pass before checking).
//!   * Helpers that consult these registries (subtyping, member lookup,
//!     in-scope generic tracking).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use saule_ast::{ClassMember, Decl, EnumVariant, Module, Stmt, Type};

// ──────────────────────────────────────────────────────────────────────────────
// Scope
// ──────────────────────────────────────────────────────────────────────────────

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

// ──────────────────────────────────────────────────────────────────────────────
// Registry types
// ──────────────────────────────────────────────────────────────────────────────

/// Class info collected by [`build_registry`] so member-access checks can
/// consult member visibility, parent classes, etc.
#[derive(Default, Clone)]
pub(super) struct ClassInfo {
    pub(super) parent: Option<String>,
    /// Interfaces declared on the class (`class C implements A, B`).
    pub(super) implements: Vec<String>,
    /// member name -> is_private
    pub(super) members: HashMap<String, bool>,
}

pub(super) type ClassRegistry = HashMap<String, ClassInfo>;
pub(super) type InterfaceRegistry = HashMap<String, Vec<String>>;

/// Enum info: variant name -> payload arity (0 for `Bare`/`Valued`,
/// N for `Tuple { fields: [...; N] }`).
#[derive(Default, Clone)]
pub(super) struct EnumInfo {
    pub(super) variants: HashMap<String, usize>,
}
pub(super) type EnumRegistry = HashMap<String, EnumInfo>;

thread_local! {
    static CLASSES: RefCell<ClassRegistry> = RefCell::new(HashMap::new());
    static INTERFACES: RefCell<InterfaceRegistry> = RefCell::new(HashMap::new());
    static ENUMS: RefCell<EnumRegistry> = RefCell::new(HashMap::new());
    static CURRENT_CLASS: RefCell<Option<String>> = const { RefCell::new(None) };

    /// Generic type-parameter names in scope for the function/method body
    /// currently being checked. Treated as `any`-equivalent so that
    /// `table<T>`, `T?`, and bare `T` accept any concrete instantiation.
    static GENERICS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

// ──────────────────────────────────────────────────────────────────────────────
// Registry accessors
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn with_classes<R>(f: impl FnOnce(&ClassRegistry) -> R) -> R {
    CLASSES.with(|c| f(&c.borrow()))
}

pub(super) fn with_interfaces<R>(f: impl FnOnce(&InterfaceRegistry) -> R) -> R {
    INTERFACES.with(|c| f(&c.borrow()))
}

pub(super) fn with_enums<R>(f: impl FnOnce(&EnumRegistry) -> R) -> R {
    ENUMS.with(|c| f(&c.borrow()))
}

/// Is `iface` a known interface name?
pub(super) fn is_interface(name: &str) -> bool {
    with_interfaces(|r| r.contains_key(name))
}

/// Does `iface` extend `target` (transitively, including itself)?
pub(super) fn interface_extends(iface: &str, target: &str) -> bool {
    if iface == target {
        return true;
    }
    with_interfaces(|r| {
        let mut stack: Vec<String> = vec![iface.to_string()];
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur.clone()) {
                continue;
            }
            if cur == target {
                return true;
            }
            if let Some(parents) = r.get(&cur) {
                for p in parents {
                    stack.push(p.clone());
                }
            }
        }
        false
    })
}

/// Does class `class` (or any ancestor) implement interface `target`
/// (directly or via interface composition)?
pub(super) fn class_implements(class: &str, target: &str) -> bool {
    with_classes(|reg| {
        let mut cur = Some(class.to_string());
        while let Some(name) = cur {
            let Some(info) = reg.get(&name) else {
                return false;
            };
            for i in &info.implements {
                if interface_extends(i, target) {
                    return true;
                }
            }
            cur = info.parent.clone();
        }
        false
    })
}

/// `got` is a subtype of `expected` when:
/// - they're equal, OR
/// - `expected` is an interface and `got` is a class implementing it
///   (directly or via a parent), OR
/// - both are interfaces and `got` extends `expected`, OR
/// - `got` is a class that extends `expected` (class inheritance).
pub(super) fn is_subtype_named(got: &str, expected: &str) -> bool {
    if got == expected {
        return true;
    }
    if is_interface(expected) && class_implements(got, expected) {
        return true;
    }
    if is_interface(got) && is_interface(expected) && interface_extends(got, expected) {
        return true;
    }
    with_classes(|reg| {
        let mut cur = reg.get(got).and_then(|i| i.parent.clone());
        while let Some(name) = cur {
            if name == expected {
                return true;
            }
            cur = reg.get(&name).and_then(|i| i.parent.clone());
        }
        false
    })
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

/// Look up `member` on `class` (walking the parent chain). Returns
/// `Some((owning_class, is_private))` if found.
pub(super) fn lookup_member(class: &str, member: &str) -> Option<(String, bool)> {
    with_classes(|reg| {
        let mut cur = Some(class.to_string());
        while let Some(name) = cur {
            if let Some(info) = reg.get(&name) {
                if let Some(&priv_) = info.members.get(member) {
                    return Some((name, priv_));
                }
                cur = info.parent.clone();
            } else {
                return None;
            }
        }
        None
    })
}

/// Does the class (or any ancestor) declare it implements `Iterable` or
/// `Iterable2`? Used by the `for ... in` static check.
pub(super) fn class_implements_iterable(class: &str) -> bool {
    class_implements(class, "Iterable") || class_implements(class, "Iterable2")
}

// ──────────────────────────────────────────────────────────────────────────────
// Registry build / install / clear
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn build_registry(
    module: &Module,
) -> (ClassRegistry, InterfaceRegistry, EnumRegistry) {
    let mut reg = ClassRegistry::new();
    let mut ifaces = InterfaceRegistry::new();
    let mut enums = EnumRegistry::new();
    for stmt in &module.stmts {
        if let Stmt::Decl(d) = &stmt.value {
            match &d.value {
                Decl::Class {
                    name,
                    extends,
                    implements,
                    members,
                    ..
                } => {
                    let mut info = ClassInfo {
                        parent: extends.clone(),
                        implements: implements.clone(),
                        members: HashMap::new(),
                    };
                    for m in members {
                        match &m.value {
                            ClassMember::Field {
                                name, is_private, ..
                            } => {
                                info.members.insert(name.clone(), *is_private);
                            }
                            ClassMember::Method(meth) => {
                                info.members.insert(meth.name.clone(), meth.is_private);
                            }
                        }
                    }
                    reg.insert(name.clone(), info);
                }
                Decl::Interface { name, extends, .. } => {
                    ifaces.insert(name.clone(), extends.clone());
                }
                Decl::Enum { name, variants, .. } => {
                    let mut info = EnumInfo::default();
                    for v in variants {
                        let (vname, arity) = match &v.value {
                            EnumVariant::Bare(n) => (n.clone(), 0),
                            EnumVariant::Valued(n, _) => (n.clone(), 0),
                            EnumVariant::Tuple { name, fields } => (name.clone(), fields.len()),
                        };
                        info.variants.insert(vname, arity);
                    }
                    enums.insert(name.clone(), info);
                }
                _ => {}
            }
        }
    }
    // Pre-register the builtin iterable interfaces so class-implements checks
    // see them even without explicit declarations in user code.
    ifaces.entry("Iterable".into()).or_default();
    ifaces.entry("Iterable2".into()).or_default();
    (reg, ifaces, enums)
}

pub(super) fn install_registries(
    reg: ClassRegistry,
    ifaces: InterfaceRegistry,
    enums: EnumRegistry,
) {
    CLASSES.with(|c| *c.borrow_mut() = reg);
    INTERFACES.with(|c| *c.borrow_mut() = ifaces);
    ENUMS.with(|c| *c.borrow_mut() = enums);
}

pub(super) fn clear_registries() {
    CLASSES.with(|c| c.borrow_mut().clear());
    INTERFACES.with(|c| c.borrow_mut().clear());
    ENUMS.with(|c| c.borrow_mut().clear());
}
