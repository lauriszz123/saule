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
    builtins: coroutine_builtins,
    auto_prelude: true,
};

/// What the checker knows about the two types, declared as the generic
/// classes a program sees:
///
/// ```text
/// class Coroutine<T> implements Iterable<T>
///   static fn create<T>(body: fn(Yielder<T>) -> nil) -> Coroutine<T>
///   fn resume() -> T?
///   fn status() -> string
///   fn isDone() -> boolean
///   fn error() -> string?
///   fn iter() -> fn() -> T?
/// end
///
/// class Yielder<T>
///   fn yield(value: T) -> nil
/// end
/// ```
///
/// Declared as classes rather than as native signatures because only the
/// class machinery substitutes a *receiver's* type arguments: `resume` on a
/// `Coroutine<integer>` answers `integer?` the way `get` on a `Box<integer>`
/// does. Neither class has a constructor or fields — a coroutine is made by
/// `create`, and its yielder by the coroutine itself.
fn coroutine_builtins() -> saule_semantic::builtins::Builtins {
    use saule_ast::Type;
    use saule_semantic::{ClassInfo, MethodSig};

    let named = |n: &str| Type::Named(n.to_string());
    let t = || named("T");
    let generic = |name: &str, arg: Type| {
        Type::Generic(Box::new(saule_ast::GenericType {
            name: name.to_string(),
            args: vec![arg],
        }))
    };
    let param = |name: &str, ty: Type| saule_ast::Param {
        name: name.to_string(),
        ty,
        default: None,
        variadic: false,
        span: 0..0,
    };
    let method = |is_static: bool, type_params: Vec<&str>, params, ret: Type| MethodSig {
        is_static,
        is_private: false,
        type_params: type_params.into_iter().map(String::from).collect(),
        type_bounds: Vec::new(),
        params,
        return_ty: Some(ret),
    };
    let class = |methods: Vec<(&str, MethodSig)>| {
        let mut info = ClassInfo {
            type_params: vec!["T".to_string()],
            ..ClassInfo::default()
        };
        for (name, sig) in methods {
            info.members.insert(name.to_string(), false);
            info.methods.insert(name.to_string(), sig);
        }
        info
    };

    let mut coroutine = class(vec![
        (
            "create",
            method(
                true,
                vec!["T"],
                vec![param(
                    "body",
                    Type::Function {
                        params: vec![generic("Yielder", t())],
                        ret: Box::new(named("nil")),
                    },
                )],
                generic("Coroutine", t()),
            ),
        ),
        (
            "resume",
            method(false, vec![], vec![], Type::Nullable(Box::new(t()))),
        ),
        ("status", method(false, vec![], vec![], named("string"))),
        ("isDone", method(false, vec![], vec![], named("boolean"))),
        (
            "error",
            method(false, vec![], vec![], Type::Nullable(Box::new(named("string")))),
        ),
        (
            "iter",
            method(
                false,
                vec![],
                vec![],
                Type::Function {
                    params: vec![],
                    ret: Box::new(Type::Nullable(Box::new(t()))),
                },
            ),
        ),
    ]);
    // `for v in gen do` — a coroutine is iterated by resuming it.
    coroutine.implements.push("Iterable".to_string());
    coroutine.implements_args.push(vec![t()]);

    let yielder = class(vec![(
        "yield",
        method(false, vec![], vec![param("value", t())], named("nil")),
    )]);

    let mut builtins = saule_semantic::builtins::Builtins::default();
    builtins
        .classes
        .insert("Coroutine".to_string(), coroutine);
    builtins.classes.insert("Yielder".to_string(), yielder);
    builtins
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

/// Native signatures: none. Both types are described to the checker as
/// classes by [`coroutine_builtins`], which is what lets a receiver's type
/// argument reach its methods.
pub fn register_sigs() {}
