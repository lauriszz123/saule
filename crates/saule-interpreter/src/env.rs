//! Lexically-scoped variable environment.
//!
//! Environments form a parent chain. Each block (function body, `if`/`while`
//! body, etc.) creates a child via [`Environment::with_parent`] and is
//! dropped when execution leaves the scope.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use crate::module::ModuleLoader;
use crate::value::Value;

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
            vars: HashMap::new(),
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

    /// Look up a name, walking parent scopes until found.
    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.vars.get(name) {
            Some(v.clone())
        } else if let Some(parent) = &self.parent {
            parent.borrow().get(name)
        } else {
            None
        }
    }

    /// Assign to an existing variable in this or an ancestor scope.
    /// Returns `false` if no such binding exists.
    pub fn assign(&mut self, name: &str, value: Value) -> bool {
        if let Some(slot) = self.vars.get_mut(name) {
            *slot = value;
            return true;
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
