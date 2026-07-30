//! Core prelude functions.

use std::rc::Rc;

use crate::env::Environment;
use crate::native_packages::NativePackage;
use crate::stdlib::{define_native, expect_arity};
use crate::value::Value;

/// `import { print, println, … } from "core"`. Also auto-installed so
/// these names are visible without an explicit import (the common
/// case — `print` is a language built-in, not a library).
pub static CORE_PACKAGE: NativePackage = NativePackage {
    name: "core",
    version: saule_version::VERSION,
    install,
    exports: &[
        "print",
        "println",
        "printf",
        "tostring",
        "type",
        "int",
        "float",
        "tonumber",
        "tointeger",
        "tofloat",
        "assert",
        "error",
    ],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

pub fn install(env: &std::rc::Rc<std::cell::RefCell<Environment>>) {
    define_native(env, "print", builtin_print);
    define_native(env, "println", builtin_println);
    define_native(env, "printf", builtin_printf);
    define_native(env, "tostring", builtin_tostring);
    define_native(env, "type", builtin_type);
    define_native(env, "int", builtin_int);
    define_native(env, "float", builtin_float);
    define_native(env, "tonumber", builtin_tonumber);
    define_native(env, "tointeger", builtin_tointeger);
    define_native(env, "tofloat", builtin_tofloat);
    define_native(env, "assert", builtin_assert);
    define_native(env, "error", builtin_error);
}

/// Register native signatures for the typechecker. Called lazily by
/// `sigs::lookup` on first use so signatures are available even before
/// `install_std` runs (typecheck runs prior to environment construction).
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, register_g, register_v, t_any, t_named, t_nullable};
    let any = t_any();
    // `print/println` accept anything, any number of times.
    register_v("print", vec![], any.clone(), vec![t_named("nil")]);
    register_v("println", vec![], any.clone(), vec![t_named("nil")]);
    // `printf(fmt, ...)` — `fmt` must be a string; extras are anything
    // (their type is decided by the spec).
    register_v(
        "printf",
        vec![t_named("string")],
        any.clone(),
        vec![t_named("nil")],
    );
    register("tostring", vec![any.clone()], vec![t_named("string")]);
    register("type", vec![any.clone()], vec![t_named("string")]);
    register("int", vec![any.clone()], vec![t_named("integer")]);
    register("float", vec![any.clone()], vec![t_named("float")]);
    // `tonumber(s)` returns `integer | float | nil` — modelled as nullable
    // `any` so callers can `force-unwrap` or `match` on the result.
    register(
        "tonumber",
        vec![any.clone()],
        vec![t_nullable(t_named("any"))],
    );
    // Strict variants: succeed only when the value is/parses as the named
    // kind, otherwise return `nil`.
    register(
        "tointeger",
        vec![any.clone()],
        vec![t_nullable(t_named("integer"))],
    );
    register(
        "tofloat",
        vec![any.clone()],
        vec![t_nullable(t_named("float"))],
    );
    // `assert<T>(v: T?, msg: string?) -> T` — strips the nullability of
    // the input on success. The generic param binds to whatever non-null
    // base type `v` has, so `local x: Foo = assert(maybeFoo)` is checked
    // against `Foo` rather than the historical `any` widening.
    register_g(
        "assert",
        vec!["T"],
        vec![t_nullable(t_named("T")), t_nullable(t_named("string"))],
        vec![t_named("T")],
    );
    let _ = any;
    register("error", vec![t_named("string")], vec![t_named("nil")]);
}

fn builtin_print(args: &[Value]) -> Result<Value, String> {
    crate::output::write(crate::output::Stream::Stdout, &display_all(args)?);
    Ok(Value::Nil)
}

fn builtin_println(args: &[Value]) -> Result<Value, String> {
    crate::output::write(
        crate::output::Stream::Stdout,
        &format!("{}\n", display_all(args)?),
    );
    Ok(Value::Nil)
}

/// Tab-join the arguments, honouring any `OpToString` overload.
fn display_all(args: &[Value]) -> Result<String, String> {
    let parts: Result<Vec<String>, String> = args
        .iter()
        .map(crate::eval::ops::display_value_native)
        .collect();
    Ok(parts?.join("\t"))
}

/// `printf(fmt, ...)` — same format spec as `String.format`, written to
/// stdout without a trailing newline.
fn builtin_printf(args: &[Value]) -> Result<Value, String> {
    let s = crate::stdlib::string::format_args_impl(args)?;
    crate::output::write(crate::output::Stream::Stdout, &s);
    Ok(Value::Nil)
}

fn builtin_tostring(args: &[Value]) -> Result<Value, String> {
    let v = args.first().cloned().unwrap_or(Value::Nil);
    Ok(Value::Str(Rc::new(crate::eval::ops::display_value_native(
        &v,
    )?)))
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

/// `tonumber(v)` — accept a number unchanged, or parse a string into
/// integer-or-float. Returns `nil` on anything else or on parse failure so
/// callers can branch with `if n != nil then`.
fn builtin_tonumber(args: &[Value]) -> Result<Value, String> {
    expect_arity("tonumber", args, 1)?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Float(*f)),
        Value::Str(s) => {
            let trimmed = s.trim();
            // Integer wins when it parses; otherwise fall back to float.
            // This keeps `tonumber("42")` strictly an integer instead of a
            // float, matching `int`/`float`'s behaviour on numeric values.
            if let Ok(i) = trimmed.parse::<i64>() {
                Ok(Value::Int(i))
            } else if let Ok(f) = trimmed.parse::<f64>() {
                Ok(Value::Float(f))
            } else {
                Ok(Value::Nil)
            }
        }
        _ => Ok(Value::Nil),
    }
}

/// `tointeger(v)` — strict integer parse / coerce.
///
/// * `integer`        → unchanged
/// * `float`          → only when it has no fractional part (e.g. `3.0`)
/// * `string`         → only when it parses as an `i64`
/// * anything else    → `nil`
fn builtin_tointeger(args: &[Value]) -> Result<Value, String> {
    expect_arity("tointeger", args, 1)?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f)
            if f.is_finite()
                && f.fract() == 0.0
                && *f >= i64::MIN as f64
                && *f <= i64::MAX as f64 =>
        {
            Ok(Value::Int(*f as i64))
        }
        Value::Str(s) => match s.trim().parse::<i64>() {
            Ok(i) => Ok(Value::Int(i)),
            Err(_) => Ok(Value::Nil),
        },
        _ => Ok(Value::Nil),
    }
}

/// `tofloat(v)` — strict float parse / coerce.
///
/// * `float`          → unchanged
/// * `integer`        → widened to `float`
/// * `string`         → only when it parses as `f64`
/// * anything else    → `nil`
fn builtin_tofloat(args: &[Value]) -> Result<Value, String> {
    expect_arity("tofloat", args, 1)?;
    match &args[0] {
        Value::Float(f) => Ok(Value::Float(*f)),
        Value::Int(n) => Ok(Value::Float(*n as f64)),
        Value::Str(s) => match s.trim().parse::<f64>() {
            Ok(f) => Ok(Value::Float(f)),
            Err(_) => Ok(Value::Nil),
        },
        _ => Ok(Value::Nil),
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

fn builtin_error(args: &[Value]) -> Result<Value, String> {
    let msg = args
        .first()
        .cloned()
        .unwrap_or_else(|| Value::Str(Rc::new("error".to_string())));
    Err(msg.to_display_string())
}
