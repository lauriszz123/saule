//! Lexically-scoped variable environment.
//!
//! Environments form a parent chain. Each block (function body, `if`/`while`
//! body, etc.) creates a child via [`Environment::with_parent`] and is
//! dropped when execution leaves the scope.

use std::cell::RefCell;
use crate::fxhash::FxHashMap as HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use crate::module::ModuleLoader;
use crate::value::{ClassObject, Value};

/// Lexical scope: a `HashMap` of locals plus an optional parent pointer.
///
/// The optional `module_dir` and `loader` fields are only populated on the
/// root scope created by [`Environment::with_prelude_and_context`]; child
/// scopes inherit them by walking the parent chain via [`module_dir`] and
/// [`loader`].
#[derive(Debug, Default)]
pub struct Environment {
    parent: Option<Rc<RefCell<Environment>>>,
    vars: HashMap<String, Value>,
    /// Set on a method-call scope so the class's statics are reachable by
    /// bare name inside the body. Consulted by [`get`](Self::get) *after*
    /// `vars` and *before* `parent`, which reproduces the precedence of
    /// eagerly copying every static into `vars` — parameters and locals
    /// shadow statics, statics shadow the enclosing closure — without
    /// paying a map insert per static per call.
    statics_owner: Option<Rc<ClassObject>>,
    module_dir: Option<PathBuf>,
    loader: Option<Rc<RefCell<ModuleLoader>>>,
}

impl Environment {
    /// Empty global scope.
    pub fn new() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::default()))
    }

    /// Child scope of `parent`.
    pub fn with_parent(parent: Rc<RefCell<Self>>) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            parent: Some(parent),
            vars: HashMap::default(),
            statics_owner: None,
            module_dir: None,
            loader: None,
        }))
    }

    /// Global scope pre-populated with the standard built-ins.
    pub fn with_prelude() -> Rc<RefCell<Self>> {
        let env = Self::new();
        crate::stdlib::install_std(&env);
        env
    }

    /// Like [`Environment::with_prelude`], but also stamps the root scope
    /// with the importing file's directory and a shared module loader so
    /// `import "..."` statements can resolve relative paths and dedupe
    /// already-loaded modules.
    pub fn with_prelude_and_context(
        module_dir: Option<PathBuf>,
        loader: Option<Rc<RefCell<ModuleLoader>>>,
    ) -> Rc<RefCell<Self>> {
        let env = Self::with_prelude();
        {
            let mut b = env.borrow_mut();
            b.module_dir = module_dir;
            b.loader = loader;
        }
        env
    }

    /// Define (or shadow) a local in this scope.
    pub fn define(&mut self, name: String, value: Value) {
        self.vars.insert(name, value);
    }

    /// Make `class`'s statics visible by bare name in this scope. See
    /// [`statics_owner`](Self::statics_owner).
    pub fn set_statics_owner(&mut self, class: Rc<ClassObject>) {
        self.statics_owner = Some(class);
    }

    /// Look up a name, walking parent scopes until found.
    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
        }
        if let Some(class) = &self.statics_owner
            && let Some(v) = class_static(class, name)
        {
            return Some(v);
        }
        match &self.parent {
            Some(parent) => parent.borrow().get(name),
            None => None,
        }
    }

    /// Assign to an existing variable in this or an ancestor scope.
    /// Returns `false` if no such binding exists.
    pub fn assign(&mut self, name: &str, value: Value) -> bool {
        if let Some(slot) = self.vars.get_mut(name) {
            *slot = value;
            return true;
        }
        // Bare-name write to one of the owning class's statics. This has to
        // reach the class itself: `static local` members are invisible from
        // outside the class, so a bare-name write is the *only* way to
        // mutate one — routing it to a scope-local copy would make them
        // silently immutable.
        match self
            .statics_owner
            .as_ref()
            .and_then(|c| resolve_static_write(c, name))
        {
            Some(StaticTarget::Field(owner)) => {
                owner.static_fields.borrow_mut().insert(name.to_string(), value);
                return true;
            }
            // Assigning to a static *method*'s name. `static_methods` is
            // immutable, so the binding can only be shadowed for the rest
            // of this scope, which is what the old eager injection did.
            Some(StaticTarget::Method) => {
                self.vars.insert(name.to_string(), value);
                return true;
            }
            None => {}
        }
        if let Some(parent) = &self.parent {
            return parent.borrow_mut().assign(name, value);
        }
        false
    }

    /// Walk the parent chain to find the nearest set `module_dir`.
    pub fn module_dir(&self) -> Option<PathBuf> {
        if self.module_dir.is_some() {
            return self.module_dir.clone();
        }
        self.parent.as_ref().and_then(|p| p.borrow().module_dir())
    }

    /// Walk the parent chain to find the nearest attached module loader.
    pub fn loader(&self) -> Option<Rc<RefCell<ModuleLoader>>> {
        if self.loader.is_some() {
            return self.loader.clone();
        }
        self.parent.as_ref().and_then(|p| p.borrow().loader())
    }
}

/// Resolve a bare name against a class's statics, walking the inheritance
/// chain from the class itself up to the root.
///
/// The per-level order (methods before fields, nearest class first) mirrors
/// exactly what eager injection produced: it seeded each class root-first,
/// writing that class's fields and then its methods into the scope, so a
/// nearer class overwrote a farther one and — within one class — a method
/// overwrote a same-named field. Colliding names are almost certainly
/// rejected earlier by `saule_semantic`, but matching the old precedence
/// costs nothing and keeps this a pure optimization.
fn class_static(class: &Rc<ClassObject>, name: &str) -> Option<Value> {
    let mut cur = Some(class);
    while let Some(c) = cur {
        if let Some(m) = c.static_methods.get(name) {
            return Some(Value::Function(m.clone()));
        }
        if let Some(v) = c.static_fields.borrow().get(name) {
            return Some(v.clone());
        }
        cur = c.parent.as_ref();
    }
    None
}

/// What a bare-name assignment inside a method body resolves to.
enum StaticTarget {
    /// A static field, held by the class in the chain that declares it.
    Field(Rc<ClassObject>),
    /// A static method — a name that exists but has no writable slot.
    Method,
}

/// Write-side counterpart to [`class_static`], using the same chain order so
/// a name always reads and writes through the same member.
fn resolve_static_write(class: &Rc<ClassObject>, name: &str) -> Option<StaticTarget> {
    let mut cur = Some(class);
    while let Some(c) = cur {
        if c.static_methods.contains_key(name) {
            return Some(StaticTarget::Method);
        }
        if c.static_fields.borrow().contains_key(name) {
            return Some(StaticTarget::Field(c.clone()));
        }
        cur = c.parent.as_ref();
    }
    None
}
