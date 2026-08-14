//! Class / interface / enum metadata extracted from a parsed module.
//!
//! Built once by [`build_registry`] at the start of [`crate::analyze`] and
//! installed into thread-local slots so later passes (the typechecker and
//! the rest of the semantic walker) can consult them without threading a
//! context through every recursion.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use saule_ast::{ClassMember, Decl, EnumVariant, Module, Param, Stmt, Type};

// ──────────────────────────────────────────────────────────────────────────────
// Registry types
// ──────────────────────────────────────────────────────────────────────────────

/// Full signature of a class method as collected from the AST. Stored so
/// `saule-typeck` can infer the return type of `obj.method(args)` and
/// validate arg counts/types without a separate scan.
#[derive(Clone, Debug)]
pub struct MethodSig {
    pub is_static: bool,
    pub is_private: bool,
    /// Generic type parameters declared with `<T, U>` after the method
    /// name. Erased at runtime; the typechecker treats these as universal
    /// inside the body and uses them to enforce caller-side unification.
    pub type_params: Vec<String>,
    pub params: Vec<Param>,
    pub return_ty: Option<Type>,
}

/// Class info collected so member-access checks can consult member
/// visibility, parent classes, etc.
#[derive(Default, Clone, Debug)]
pub struct ClassInfo {
    pub parent: Option<String>,
    /// Interfaces declared on the class (`class C implements A, B`).
    pub implements: Vec<String>,
    /// member name -> is_private. Kept distinct from [`methods`] so the
    /// existing visibility check can stay O(1) when the member is a field.
    pub members: HashMap<String, bool>,
    /// Field-name -> declared type. Populated for `local`/static fields so
    /// the typechecker can infer `self.foo` / `instance.foo` and enforce
    /// assignment compatibility for indexed writes through fields.
    pub field_types: HashMap<String, Type>,
    /// Method-name -> full signature. Populated for every `fn` declared
    /// on the class so the typechecker can answer "what does
    /// `Foo.bar(...)` return?".
    pub methods: HashMap<String, MethodSig>,
}

pub type ClassRegistry = HashMap<String, ClassInfo>;
pub type InterfaceRegistry = HashMap<String, Vec<String>>;

/// One variant's payload shape.
///
/// Empty for `Bare` and `Valued`; for `Tuple { fields }` it is the declared
/// fields in order. Keeping the whole [`Param`] rather than just its type is
/// what lets a match arm bind `case Shape.Rect(w, h)` at the declared types
/// instead of `any`, and leaves the names available for diagnostics.
#[derive(Default, Clone, Debug)]
pub struct VariantInfo {
    pub fields: Vec<Param>,
}

impl VariantInfo {
    /// How many values a pattern for this variant must bind.
    pub fn arity(&self) -> usize {
        self.fields.len()
    }

    /// The declared type of payload slot `at`.
    pub fn field_ty(&self, at: usize) -> Option<&Type> {
        self.fields.get(at).map(|f| &f.ty)
    }
}

/// Enum info: variant name -> payload shape.
#[derive(Default, Clone, Debug)]
pub struct EnumInfo {
    pub variants: HashMap<String, VariantInfo>,
}
pub type EnumRegistry = HashMap<String, EnumInfo>;

/// Full signature of a top-level `fn`. The typechecker keeps its own copy
/// for the module being checked; this registry exists so the *imported*
/// ones have a signature too. Without it `atLeast(a, b)` — declared in
/// another file — infers as "no type at all", and every checked position
/// it feeds fails with `UndeterminedType`.
#[derive(Clone, Debug)]
pub struct FunctionSig {
    /// Generic type parameters declared with `<T, U>` after the name.
    pub type_params: Vec<String>,
    pub params: Vec<Param>,
    pub return_ty: Option<Type>,
}

pub type FunctionRegistry = HashMap<String, FunctionSig>;

/// Declared types of module-level `export name: T = value` variables,
/// keyed by the name they are visible under locally (so an
/// `import x as y` entry is filed under `y`).
pub type VariableRegistry = HashMap<String, Type>;

thread_local! {
    static CLASSES: RefCell<ClassRegistry> = RefCell::new(HashMap::new());
    static INTERFACES: RefCell<InterfaceRegistry> = RefCell::new(HashMap::new());
    static ENUMS: RefCell<EnumRegistry> = RefCell::new(HashMap::new());
    static FUNCTIONS: RefCell<FunctionRegistry> = RefCell::new(HashMap::new());
    static VARIABLES: RefCell<VariableRegistry> = RefCell::new(HashMap::new());
}

// ──────────────────────────────────────────────────────────────────────────────
// Registry accessors
// ──────────────────────────────────────────────────────────────────────────────

pub fn with_classes<R>(f: impl FnOnce(&ClassRegistry) -> R) -> R {
    CLASSES.with(|c| f(&c.borrow()))
}

pub fn with_interfaces<R>(f: impl FnOnce(&InterfaceRegistry) -> R) -> R {
    INTERFACES.with(|c| f(&c.borrow()))
}

pub fn with_enums<R>(f: impl FnOnce(&EnumRegistry) -> R) -> R {
    ENUMS.with(|c| f(&c.borrow()))
}

pub fn with_functions<R>(f: impl FnOnce(&FunctionRegistry) -> R) -> R {
    FUNCTIONS.with(|c| f(&c.borrow()))
}

pub fn with_variables<R>(f: impl FnOnce(&VariableRegistry) -> R) -> R {
    VARIABLES.with(|c| f(&c.borrow()))
}

/// Look up a top-level function's signature by its *local* name — the
/// alias an `import ... as ...` bound it to, when there is one.
pub fn lookup_function(name: &str) -> Option<FunctionSig> {
    with_functions(|reg| reg.get(name).cloned())
}

/// Is `iface` a known interface name?
pub fn is_interface(name: &str) -> bool {
    with_interfaces(|r| r.contains_key(name))
}

/// Does `iface` extend `target` (transitively, including itself)?
pub fn interface_extends(iface: &str, target: &str) -> bool {
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
pub fn class_implements(class: &str, target: &str) -> bool {
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
pub fn is_subtype_named(got: &str, expected: &str) -> bool {
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

/// Look up `member` on `class` (walking the parent chain). Returns
/// `Some((owning_class, is_private))` if found.
pub fn lookup_member(class: &str, member: &str) -> Option<(String, bool)> {
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

/// Look up a method's full signature on `class` (walking the parent
/// chain). Returns `None` for fields or unknown names.
pub fn lookup_method(class: &str, method: &str) -> Option<MethodSig> {
    with_classes(|reg| {
        let mut cur = Some(class.to_string());
        while let Some(name) = cur {
            let info = reg.get(&name)?;
            if let Some(sig) = info.methods.get(method) {
                return Some(sig.clone());
            }
            cur = info.parent.clone();
        }
        None
    })
}

/// Resolve what a `self.super(...)` written inside `class` delegates to:
/// the nearest ancestor that actually declares an `init`, paired with its
/// signature. Mirrors the interpreter's `constructor_chain`, which walks
/// the same chain from the parent up. `None` when `class` is unknown, has
/// no parent, or no ancestor declares a constructor.
pub fn super_init_target(class: &str) -> Option<(String, MethodSig)> {
    with_classes(|reg| {
        let mut cur = reg.get(class)?.parent.clone();
        while let Some(name) = cur {
            let info = reg.get(&name)?;
            if let Some(sig) = info.methods.get("init") {
                return Some((name, sig.clone()));
            }
            cur = info.parent.clone();
        }
        None
    })
}

/// Look up the declared type of field `name` on `class` (walking the parent
/// chain). Returns `None` for methods or unknown names.
pub fn lookup_field_type(class: &str, name: &str) -> Option<Type> {
    with_classes(|reg| {
        let mut cur = Some(class.to_string());
        while let Some(cname) = cur {
            let info = reg.get(&cname)?;
            if let Some(ty) = info.field_types.get(name) {
                return Some(ty.clone());
            }
            cur = info.parent.clone();
        }
        None
    })
}

/// Does the class (or any ancestor) declare it implements `Iterable` or
/// `Iterable2`? Used by the `for ... in` static check.
pub fn class_implements_iterable(class: &str) -> bool {
    class_implements(class, "Iterable") || class_implements(class, "Iterable2")
}

// ──────────────────────────────────────────────────────────────────────────────
// Registry build / install / clear
// ──────────────────────────────────────────────────────────────────────────────

pub fn build_registry(module: &Module) -> (ClassRegistry, InterfaceRegistry, EnumRegistry) {
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
                        field_types: HashMap::new(),
                        methods: HashMap::new(),
                    };
                    for m in members {
                        match &m.value {
                            ClassMember::Field {
                                name,
                                is_private,
                                ty,
                                ..
                            } => {
                                info.members.insert(name.clone(), *is_private);
                                info.field_types.insert(name.clone(), ty.clone());
                            }
                            ClassMember::Method(meth) => {
                                info.members.insert(meth.name.clone(), meth.is_private);
                                info.methods.insert(
                                    meth.name.clone(),
                                    MethodSig {
                                        is_static: meth.is_static,
                                        is_private: meth.is_private,
                                        type_params: meth.type_params.clone(),
                                        params: meth.params.clone(),
                                        return_ty: meth.return_ty.clone(),
                                    },
                                );
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
                        let (vname, fields) = match &v.value {
                            EnumVariant::Bare(n) => (n.clone(), Vec::new()),
                            EnumVariant::Valued(n, _) => (n.clone(), Vec::new()),
                            EnumVariant::Tuple { name, fields } => (name.clone(), fields.clone()),
                        };
                        info.variants.insert(vname, VariantInfo { fields });
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
    // Same for the built-in contracts, so `implements OpAdd<…>` and
    // `implements Assignable<…>` resolve without an import.
    for contract in saule_ast::ops::ALL_CONTRACTS {
        ifaces.entry(contract.interface.to_string()).or_default();
    }
    (reg, ifaces, enums)
}

/// Top-level `fn` signatures declared directly in `module`. Kept separate
/// from [`build_registry`] so the existing three-tuple callers don't have
/// to change.
pub fn build_function_registry(module: &Module) -> FunctionRegistry {
    let mut out = FunctionRegistry::new();
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
            out.insert(
                name.clone(),
                FunctionSig {
                    type_params: type_params.clone(),
                    params: params.clone(),
                    return_ty: return_ty.clone(),
                },
            );
        }
    }
    out
}

pub fn install_registries(reg: ClassRegistry, ifaces: InterfaceRegistry, enums: EnumRegistry) {
    CLASSES.with(|c| *c.borrow_mut() = reg);
    INTERFACES.with(|c| *c.borrow_mut() = ifaces);
    ENUMS.with(|c| *c.borrow_mut() = enums);
}

pub fn install_functions(funcs: FunctionRegistry) {
    FUNCTIONS.with(|c| *c.borrow_mut() = funcs);
}

pub fn install_variables(vars: VariableRegistry) {
    VARIABLES.with(|c| *c.borrow_mut() = vars);
}

/// Declared types of the module's own `export name: T` variables. An
/// un-annotated variable contributes nothing — there is no declared type
/// to record, and inference at the use site is left to fall through.
pub fn build_variable_registry(module: &Module) -> VariableRegistry {
    let mut out = VariableRegistry::new();
    for stmt in &module.stmts {
        if let Stmt::Decl(d) = &stmt.value
            && let Decl::Variable {
                name, ty: Some(t), ..
            } = &d.value
        {
            out.insert(name.clone(), t.clone());
        }
    }
    out
}

pub fn clear_registries() {
    CLASSES.with(|c| c.borrow_mut().clear());
    INTERFACES.with(|c| c.borrow_mut().clear());
    ENUMS.with(|c| c.borrow_mut().clear());
    FUNCTIONS.with(|c| c.borrow_mut().clear());
    VARIABLES.with(|c| c.borrow_mut().clear());
}
