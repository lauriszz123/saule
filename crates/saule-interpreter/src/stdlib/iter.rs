//! Built-in iteration interfaces.
//!
//! `Iterable<T>` and `Iterable2<K, V>` are recognised by `for ... in` to
//! drive user-defined classes through their `iter()` method. The method
//! returns a closure that yields the next value(s) — or `nil` to stop.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::env::Environment;
use crate::value::{InterfaceObject, Value};

pub fn install(env: &Rc<RefCell<Environment>>) {
    define_interface(env, "Iterable");
    define_interface(env, "Iterable2");
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

