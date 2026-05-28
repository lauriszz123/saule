//! Built-in iteration interfaces.
//!
//! * `Iterable<T>` and `Iterable2<K, V>` — interface contracts recognised by
//!   `for ... in instance do ... end`. Each implementer's `iter()` returns
//!   the step closure that drives the loop.
//!
//! For ad-hoc iteration over a `table`, the loop itself accepts the table
//! directly (`for v in t do` / `for k, v in t do`); no helper is needed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::env::Environment;
use crate::value::{InterfaceObject, Value};

pub fn install(env: &Rc<RefCell<Environment>>) {
    define_interface(env, "Iterable");
    define_interface(env, "Iterable2");
}

/// Register native signatures for the typechecker (lazy, via `sigs::lookup`).
pub fn register_sigs() {
    // Nothing — the `Iterable` / `Iterable2` interfaces don't have native
    // call sites the typechecker needs to know about beyond their declared
    // method signatures, which are seeded in `install`.
}

fn define_interface(env: &Rc<RefCell<Environment>>, name: &str) {
    let mut methods = HashMap::new();
    // `iter()` — zero parameters, has a return type.
    methods.insert("iter".to_string(), (0, true));
    env.borrow_mut().define(
        name.to_string(),
        Value::Interface(Rc::new(InterfaceObject {
            name: name.to_string(),
            extends: Vec::new(),
            methods,
        })),
    );
}