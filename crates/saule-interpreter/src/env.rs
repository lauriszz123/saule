//! Lexically-scoped variable environment.
//!
//! Environments form a parent chain. Each block (function body, `if`/`while`
//! body, etc.) creates a child via [`Environment::with_parent`] and is
//! dropped when execution leaves the scope.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::value::Value;

/// Lexical scope: a `HashMap` of locals plus an optional parent pointer.
#[derive(Debug, Default)]
pub struct Environment {
    parent: Option<Rc<RefCell<Environment>>>,
    vars: HashMap<String, Value>,
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
        }))
    }

    /// Global scope pre-populated with the standard built-ins.
    pub fn with_prelude() -> Rc<RefCell<Self>> {
        let env = Self::new();
        crate::builtins::install(&env);
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
}
