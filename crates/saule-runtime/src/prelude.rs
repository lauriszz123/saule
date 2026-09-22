//! The names every program starts with.
//!
//! `print`, `Math`, `Table`, `Iterable`, … — the standard library, and every
//! native package registered to join it. The compiler resolves a prelude
//! name against this at compile time and folds it into a constant, so a
//! running program never looks one up by name.

use std::cell::RefCell;
use std::rc::Rc;

use crate::fxhash::FxHashMap as HashMap;
use crate::value::Value;

/// A name → value table the standard library and native packages install
/// into.
#[derive(Debug, Default)]
pub struct Prelude {
    values: HashMap<Rc<str>, Value>,
}

impl Prelude {
    /// An empty table — what a native package's `install` fills when its
    /// exports are harvested for an `import`.
    pub fn new() -> Rc<RefCell<Prelude>> {
        Rc::new(RefCell::new(Prelude::default()))
    }

    /// The standard prelude: every `auto_prelude` package installed.
    pub fn with_std() -> Rc<RefCell<Prelude>> {
        let prelude = Prelude::new();
        crate::stdlib::install_std(&prelude);
        prelude
    }

    /// Bind `name`, replacing any earlier binding.
    pub fn define(&mut self, name: impl Into<Rc<str>>, value: Value) {
        self.values.insert(name.into(), value);
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        self.values.get(name).cloned()
    }
}
