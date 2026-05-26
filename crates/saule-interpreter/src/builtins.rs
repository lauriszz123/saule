//! Built-in functions exposed to Saule programs.
//!
//! Registered into a fresh [`Environment`](crate::Environment) by
//! [`install`].

use std::rc::Rc;

use crate::env::Environment;
use crate::value::{NativeFn, Value};

/// Define every built-in into `env`. Called by `Environment::with_prelude`.
pub fn install(env: &std::rc::Rc<std::cell::RefCell<Environment>>) {
    define(env, "print", builtin_print);
    define(env, "tostring", builtin_tostring);
    define(env, "type", builtin_type);
}

fn define(
    env: &std::rc::Rc<std::cell::RefCell<Environment>>,
    name: &'static str,
    func: fn(&[Value]) -> Result<Value, String>,
) {
    env.borrow_mut().define(
        name.to_string(),
        Value::Native(Rc::new(NativeFn { name, func })),
    );
}

fn builtin_print(args: &[Value]) -> Result<Value, String> {
    let parts: Vec<String> = args.iter().map(|v| v.to_display_string()).collect();
    println!("{}", parts.join("\t"));
    Ok(Value::Nil)
}

fn builtin_tostring(args: &[Value]) -> Result<Value, String> {
    let v = args.first().cloned().unwrap_or(Value::Nil);
    Ok(Value::Str(Rc::new(v.to_display_string())))
}

fn builtin_type(args: &[Value]) -> Result<Value, String> {
    let v = args.first().cloned().unwrap_or(Value::Nil);
    // `type(obj)` reports the class name for instances so user code can
    // dispatch on it. Other values fall back to their primitive type tag.
    let name = match &v {
        Value::Instance(inst) => inst.borrow().class.name.clone(),
        _ => v.type_name().to_string(),
    };
    Ok(Value::Str(Rc::new(name)))
}
