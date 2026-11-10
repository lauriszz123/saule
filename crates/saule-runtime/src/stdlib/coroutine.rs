//! `Coroutine` static class — making coroutines.
//!
//! Methods:
//!   * `Coroutine.create(body) -> Coroutine` — a coroutine that has not
//!     started; the first `resume` calls `body(yielder, ...args)`.
//!
//! Everything a coroutine *does* — `resume`, `status`, `isDone`, `error`, and
//! its yielder's `yield` — is a method on the value itself, dispatched to the
//! VM through [`VmCoroutine::call_method`](crate::value::VmCoroutine). Only
//! creation lives here, because it is the one operation that starts from a
//! function rather than from a coroutine.

use std::cell::RefCell;
use std::rc::Rc;

use crate::fxhash::fxmap;
use crate::native_packages::NativePackage;
use crate::prelude::Prelude;
use crate::value::{ClassObject, NativeClosure, Value};

/// Auto-prelude'd: a coroutine is a language facility, not a library one.
pub static COROUTINE_PACKAGE: NativePackage = NativePackage {
    name: "coroutine",
    version: saule_version::VERSION,
    install,
    exports: &["Coroutine"],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

pub fn install(env: &Rc<RefCell<Prelude>>) {
    let mut static_fields = fxmap();
    static_fields.insert(
        "create".to_string(),
        Value::NativeClosure(Rc::new(NativeClosure {
            name: "Coroutine.create",
            func: Box::new(create),
            param_names: vec!["body".to_string()],
        })),
    );
    let class = ClassObject {
        name: "Coroutine".to_string(),
        parent: None,
        // Statics only — the namespace is never instantiated; coroutines
        // themselves are `Value::Coroutine`, not instances of this class.
        layout: Default::default(),
        methods: Default::default(),
        static_fields: RefCell::new(static_fields),
        slot_statics: None,
        static_methods: Default::default(),
    };
    env.borrow_mut()
        .define("Coroutine".to_string(), Value::Class(Rc::new(class)));
}

/// `Coroutine.create(body)`.
///
/// Only a function compiled to bytecode can be a body: suspending means
/// keeping the function's frames, and only the VM has frames to keep. A
/// native function has a Rust stack instead, which cannot be put aside.
fn create(args: &[Value]) -> Result<Vec<Value>, String> {
    match args {
        [Value::VmFunction(f)] => match f.coroutine(f) {
            Some(co) => Ok(vec![Value::Coroutine(co)]),
            None => Err("Coroutine.create: this function cannot be run as a coroutine".into()),
        },
        [Value::Native(_) | Value::NativeClosure(_)] => Err(
            "Coroutine.create expects a Saule function — a built-in function cannot be \
             suspended, so it cannot be a coroutine's body"
                .into(),
        ),
        [other] => Err(format!(
            "Coroutine.create expects a function, got `{}`",
            other.type_name()
        )),
        _ => Err(format!(
            "Coroutine.create expects exactly 1 argument, got {}",
            args.len()
        )),
    }
}

/// Register native signatures for the typechecker.
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, t_any};
    register("Coroutine.create", vec![t_any()], vec![t_any()]);
}
