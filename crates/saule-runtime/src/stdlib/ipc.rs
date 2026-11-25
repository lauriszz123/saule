//! `Looper` — answering calls from other processes — and the internal
//! `Ipc` class that `Iface.connect` / `Iface.publish` compile to.
//!
//! `Looper`:
//!   * `Looper.dispatch(timeout: integer?) -> integer` — answer the calls
//!     that have arrived, waiting up to `timeout` milliseconds for one
//!     (`nil`: until one does). Returns how many were handled. A display
//!     server calls this once per frame.
//!   * `Looper.run()` — answer calls until `Looper.stop()`. A service with
//!     nothing else to do calls this and nothing else.
//!   * `Looper.stop()` — make `run` return.
//!   * `Looper.callerPid() -> integer?` — inside a method answering a call,
//!     the process id of the caller, as the kernel reports it.
//!
//! `Ipc` is not for programs to call: it has no signatures, so the checker
//! does not know it. `Display.connect("x")` compiles to
//! `Ipc.connect(<description of Display>, "x")`, where the description is
//! what [`crate::ipc_desc`] reads.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::fxhash::fxmap;
use crate::ipc::lang;
use crate::native_packages::NativePackage;
use crate::prelude::Prelude;
use crate::value::{ClassObject, NativeClosure, Value};

pub static IPC_PACKAGE: NativePackage = NativePackage {
    name: "ipc",
    version: saule_version::VERSION,
    install,
    exports: &["Looper"],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

fn native(name: &'static str, func: fn(&[Value]) -> Result<Vec<Value>, String>) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func: Box::new(func),
        param_names: Vec::new(),
    }))
}

fn static_class(name: &str, members: &[(&str, Value)]) -> Value {
    let mut statics = fxmap();
    for (n, v) in members {
        statics.insert((*n).to_string(), v.clone());
    }
    Value::Class(Rc::new(ClassObject {
        interfaces: Vec::new(),
        name: name.to_string(),
        parent: None,
        layout: Default::default(),
        methods: Default::default(),
        static_fields: RefCell::new(statics),
        slot_statics: None,
        static_methods: Default::default(),
    }))
}

pub fn install(env: &Rc<RefCell<Prelude>>) {
    let looper = static_class(
        "Looper",
        &[
            ("dispatch", native("Looper.dispatch", looper_dispatch)),
            ("run", native("Looper.run", looper_run)),
            ("stop", native("Looper.stop", looper_stop)),
            ("callerPid", native("Looper.callerPid", looper_caller_pid)),
        ],
    );
    let ipc = static_class(
        "Ipc",
        &[
            ("connect", native("Ipc.connect", ipc_connect)),
            ("publish", native("Ipc.publish", ipc_publish)),
        ],
    );
    let mut env = env.borrow_mut();
    env.define("Looper".to_string(), looper);
    env.define("Ipc".to_string(), ipc);
}

fn str_arg<'a>(args: &'a [Value], i: usize, func: &str) -> Result<&'a str, String> {
    match args.get(i) {
        Some(Value::Str(s)) => Ok(s.as_str()),
        Some(other) => Err(super::wrong_arg(func, i, "a `string`", other)),
        None => Err(super::missing_arg(func, i)),
    }
}

fn looper_dispatch(args: &[Value]) -> Result<Vec<Value>, String> {
    let timeout = match args.first() {
        None | Some(Value::Nil) => None,
        Some(Value::Int(ms)) if *ms >= 0 => Some(Duration::from_millis(*ms as u64)),
        Some(Value::Int(_)) => return Err("`Looper.dispatch`: the timeout cannot be negative".into()),
        Some(other) => return Err(super::wrong_arg("Looper.dispatch", 0, "an `integer`", other)),
    };
    Ok(vec![Value::Int(lang::dispatch(timeout)? as i64)])
}

fn looper_run(_: &[Value]) -> Result<Vec<Value>, String> {
    lang::run()?;
    Ok(Vec::new())
}

fn looper_stop(_: &[Value]) -> Result<Vec<Value>, String> {
    lang::stop();
    Ok(Vec::new())
}

fn looper_caller_pid(_: &[Value]) -> Result<Vec<Value>, String> {
    Ok(vec![lang::caller_pid().map_or(Value::Nil, |p| Value::Int(p as i64))])
}

fn ipc_connect(args: &[Value]) -> Result<Vec<Value>, String> {
    let desc = str_arg(args, 0, "Ipc.connect")?;
    let name = str_arg(args, 1, "connect")?;
    Ok(vec![lang::connect(desc, name)?])
}

fn ipc_publish(args: &[Value]) -> Result<Vec<Value>, String> {
    let desc = str_arg(args, 0, "Ipc.publish")?;
    let name = str_arg(args, 1, "publish")?;
    let service = args.get(2).ok_or_else(|| super::missing_arg("publish", 1))?;
    lang::publish(desc, name, service)?;
    Ok(Vec::new())
}

/// Signatures for `Looper` only; `Ipc` stays unknown to the checker.
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, t_named, t_nullable};
    let i = || t_named("integer");
    let nil = || t_named("nil");
    register("Looper.dispatch", vec![t_nullable(i())], vec![i()]);
    register("Looper.run", vec![], vec![nil()]);
    register("Looper.stop", vec![], vec![nil()]);
    register("Looper.callerPid", vec![], vec![t_nullable(i())]);
}
