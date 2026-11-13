//! Core prelude functions.

use crate::native_packages::NativePackage;
use crate::prelude::Prelude;
use crate::stdlib::define_native;
use crate::value::SauleStr;
use crate::value::Value;

/// `import { print, println, … } from "core"`. Also auto-installed so
/// these names are visible without an explicit import (the common
/// case — `print` is a language built-in, not a library).
pub static CORE_PACKAGE: NativePackage = NativePackage {
    name: "core",
    version: saule_version::VERSION,
    install,
    exports: &[
        "print", "println", "printf", "tostring", "type", "assert", "error",
    ],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

pub fn install(env: &std::rc::Rc<std::cell::RefCell<Prelude>>) {
    define_native(env, "print", builtin_print);
    define_native(env, "println", builtin_println);
    define_native(env, "printf", builtin_printf);
    define_native(env, "tostring", builtin_tostring);
    define_native(env, "type", builtin_type);
    define_native(env, "assert", builtin_assert);
    define_native(env, "error", builtin_error);
}

/// Register native signatures for the typechecker. Called lazily by
/// `sigs::lookup` on first use so signatures are available even before
/// `install_std` runs (typecheck runs before any prelude is built).
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
    // No `int` / `float` / `tonumber` / `tointeger` / `tofloat`: numeric
    // conversion is the `as` cast, which needs no signature here because
    // the typechecker knows the pairs itself (`saule_typeck::casts`).
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
    // `error(msg, level?)`: `level` says whose line the error is reported at,
    // as in Lua — 1 (the default) the function calling `error`, 2 its caller.
    register(
        "error",
        vec![t_named("string"), t_nullable(t_named("integer"))],
        vec![t_named("nil")],
    );
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
    let parts: Result<Vec<String>, String> =
        args.iter().map(crate::ops::display_value_native).collect();
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
    Ok(Value::Str(SauleStr::new(crate::ops::display_value_native(
        &v,
    )?)))
}

fn builtin_type(args: &[Value]) -> Result<Value, String> {
    let v = args.first().cloned().unwrap_or(Value::Nil);
    let name = match &v {
        Value::Instance(inst) => inst.borrow().class.name.clone(),
        // A native package's object answers the same way a Saule instance
        // does: with its class.
        Value::Foreign(obj) => obj.class.name.clone(),
        _ => v.type_name().to_string(),
    };
    Ok(Value::Str(SauleStr::new(name)))
}

fn builtin_assert(args: &[Value]) -> Result<Value, String> {
    if args.is_empty() {
        return Err("assert expects at least 1 argument".to_string());
    }
    // The *checked value*, never the message. `assert<T>(v: T?, msg) -> T`
    // is what the typechecker was told, so handing back `msg` here put a
    // string into whatever slot `T` was — a hole the checker could not see.
    if args[0].is_truthy() {
        return Ok(args[0].clone());
    }

    let message = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| Value::Str(SauleStr::new("assertion failed".to_string())));
    Err(format!("assertion failed: {}", message.to_display_string()))
}

thread_local! {
    /// The level the last `error(msg, level)` asked to be blamed at, waiting
    /// for the VM to collect it with [`take_raised_level`].
    static RAISED_LEVEL: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// Whether the native failure just returned came from `error()` and, if so,
/// at which level: 1 is the function that called `error`, 2 its caller, and
/// so on. Taking it clears it.
///
/// A native can only fail with a `String`, which leaves no room to say
/// "this was raised on purpose, blame my caller". So `error` leaves the level
/// here as it fails, and the VM, which is the only thing that knows where
/// each level's call is, reads it back when it turns the `String` into a
/// [`crate::error::RuntimeError`].
pub fn take_raised_level() -> Option<u32> {
    RAISED_LEVEL.with(|l| l.take())
}

fn builtin_error(args: &[Value]) -> Result<Value, String> {
    let msg = args
        .first()
        .cloned()
        .unwrap_or_else(|| Value::Str(SauleStr::new("error".to_string())));
    let level = match args.get(1) {
        None | Some(Value::Nil) => 1,
        Some(Value::Int(n)) if *n >= 1 => u32::try_from(*n).unwrap_or(u32::MAX),
        Some(Value::Int(n)) => {
            return Err(format!(
                "error level must be 1 or more (1 blames the function calling `error`, 2 its caller), got {n}"
            ));
        }
        Some(other) => {
            return Err(format!(
                "error level must be an `integer`, got `{}`",
                other.type_name()
            ));
        }
    };
    RAISED_LEVEL.with(|l| l.set(Some(level)));
    Err(msg.to_display_string())
}
