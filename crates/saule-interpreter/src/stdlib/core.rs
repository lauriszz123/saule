//! Core prelude functions.

use std::rc::Rc;

use crate::env::Environment;
use crate::stdlib::{define_native, expect_arity};
use crate::value::Value;

pub fn install(env: &std::rc::Rc<std::cell::RefCell<Environment>>) {
    define_native(env, "print", builtin_print);
    define_native(env, "println", builtin_println);
    define_native(env, "tostring", builtin_tostring);
    define_native(env, "type", builtin_type);
    define_native(env, "int", builtin_int);
    define_native(env, "float", builtin_float);
    define_native(env, "assert", builtin_assert);
}

fn builtin_print(args: &[Value]) -> Result<Value, String> {
    let parts: Vec<String> = args.iter().map(|v| v.to_display_string()).collect();
    println!("{}", parts.join("\t"));
    Ok(Value::Nil)
}

fn builtin_println(args: &[Value]) -> Result<Value, String> {
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
    let name = match &v {
        Value::Instance(inst) => inst.borrow().class.name.clone(),
        _ => v.type_name().to_string(),
    };
    Ok(Value::Str(Rc::new(name)))
}

fn builtin_int(args: &[Value]) -> Result<Value, String> {
    expect_arity("int", args, 1)?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Int(f.trunc() as i64)),
        other => Err(format!(
            "int expects integer or float, got `{}`",
            other.type_name()
        )),
    }
}

fn builtin_float(args: &[Value]) -> Result<Value, String> {
    expect_arity("float", args, 1)?;
    match &args[0] {
        Value::Float(f) => Ok(Value::Float(*f)),
        Value::Int(n) => Ok(Value::Float(*n as f64)),
        other => Err(format!(
            "float expects integer or float, got `{}`",
            other.type_name()
        )),
    }
}

fn builtin_assert(args: &[Value]) -> Result<Value, String> {
    if args.is_empty() {
        return Err("assert expects at least 1 argument".to_string());
    }
    if args[0].is_truthy() {
        return Ok(args.get(1).cloned().unwrap_or_else(|| args[0].clone()));
    }

    let message = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| Value::Str(Rc::new("assertion failed".to_string())));
    Err(format!("assertion failed: {}", message.to_display_string()))
}
