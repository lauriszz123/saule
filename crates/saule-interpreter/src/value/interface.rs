//! `interface` declarations.

use crate::fxhash::FxHashMap as HashMap;

/// Runtime representation of an `interface` declaration.
///
/// Carries the interface's method signatures. Used for compile-time and
/// runtime verification that implementing classes have the required methods.
#[derive(Debug)]
pub struct InterfaceObject {
    pub name: String,
    /// Parent interfaces (for interface extension).
    pub extends: Vec<String>,
    /// Method signatures required by this interface.
    /// Key is method name, value is (param_count, has_return_type).
    pub methods: HashMap<String, (usize, bool)>,
}
