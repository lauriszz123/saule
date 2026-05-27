//! Standard library installation and module wiring.
//!
//! The stdlib is installed into the prelude as plain global functions/values.
//! Namespacing (e.g. `math.abs`) can be layered on later once runtime module
//! objects are added.

use std::cell::RefCell;
use std::rc::Rc;

use crate::env::Environment;
use crate::value::{NativeFn, Value};

pub mod core;
pub mod io;
pub mod iter;
pub mod math;
pub mod os;
pub mod project;
pub mod sigs;
pub mod string;
pub mod table;

/// Install the full standard library into `env`.
pub fn install_std(env: &Rc<RefCell<Environment>>) {
    core::install(env);
    iter::install(env);
    math::install(env);
    string::install(env);
    table::install(env);
    io::install(env);
    os::install(env);
    project::install(env);
}

pub(crate) fn define_native(
    env: &Rc<RefCell<Environment>>,
    name: &'static str,
    func: fn(&[Value]) -> Result<Value, String>,
) {
    env.borrow_mut().define(
        name.to_string(),
        Value::Native(Rc::new(NativeFn { name, func })),
    );
}

pub(crate) fn expect_arity(name: &str, args: &[Value], expected: usize) -> Result<(), String> {
    if args.len() != expected {
        return Err(format!(
            "{name} expects exactly {expected} argument{}, got {}",
            if expected == 1 { "" } else { "s" },
            args.len()
        ));
    }
    Ok(())
}

pub(crate) fn expect_min_arity(name: &str, args: &[Value], min: usize) -> Result<(), String> {
    if args.len() < min {
        return Err(format!(
            "{name} expects at least {min} argument{}, got {}",
            if min == 1 { "" } else { "s" },
            args.len()
        ));
    }
    Ok(())
}


