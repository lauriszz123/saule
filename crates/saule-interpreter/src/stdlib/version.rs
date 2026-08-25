//! `Saule` static class — the toolchain's own version, visible to Saule code.
//!
//! Fields:
//!   * `Saule.version: string`  — `"26.7"`; the version as a version
//!   * `Saule.full: string`     — `"26.7"`, or `"26.8-dev+1a2b3c4"`
//!   * `Saule.year: integer`    — `26`
//!   * `Saule.build: integer`   — `7`
//!   * `Saule.isDev: boolean`   — true unless built from a clean release tag
//!   * `Saule.commit: string`   — short hash, or `""` when git was unavailable
//!
//! Methods:
//!   * `Saule.atLeast(version: string) -> boolean`
//!
//! `atLeast` exists because `min_saule_version` in `saule.config` is
//! all-or-nothing — it refuses to run the project at all. Code that wants to
//! *use* a newer facility when it's there and fall back otherwise needs a
//! runtime question, and it must be answered by the same comparator the
//! config check uses, which is why both call
//! [`saule_version::version_at_least`].
//!
//! Distinct from [`crate::stdlib::project`]: `Project.version` is the
//! version of the code being run, `Saule.version` is the version of the
//! thing running it.

use crate::fxhash::fxmap;
use std::cell::RefCell;
use std::rc::Rc;

use crate::env::Environment;
use crate::native_packages::NativePackage;
use crate::value::{ClassObject, FieldDef, NativeClosure, SauleStr, Value};

/// `import Saule from "saule"`. Auto-prelude'd, like every other stdlib
/// static class — a version check shouldn't need an import.
pub static VERSION_PACKAGE: NativePackage = NativePackage {
    name: "saule",
    version: saule_version::VERSION,
    install,
    exports: &["Saule"],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

pub fn install(env: &Rc<RefCell<Environment>>) {
    let mut static_fields = fxmap();

    static_fields.insert(
        "version".to_string(),
        Value::Str(SauleStr::new(saule_version::VERSION.to_string())),
    );
    static_fields.insert(
        "full".to_string(),
        Value::Str(SauleStr::new(saule_version::FULL.to_string())),
    );
    static_fields.insert(
        "year".to_string(),
        Value::Int(i64::from(saule_version::YEAR)),
    );
    static_fields.insert(
        "build".to_string(),
        Value::Int(i64::from(saule_version::BUILD)),
    );
    static_fields.insert("isDev".to_string(), Value::Bool(saule_version::IS_DEV));
    static_fields.insert(
        "commit".to_string(),
        Value::Str(SauleStr::new(saule_version::COMMIT.to_string())),
    );
    static_fields.insert(
        "atLeast".to_string(),
        native("Saule.atLeast", saule_at_least),
    );

    let class = ClassObject {
        name: "Saule".to_string(),
        parent: None,
        field_defs: Vec::<FieldDef>::new(),
        // Statics only — a stdlib namespace class is never instantiated.
        layout: Default::default(),
        methods: Default::default(),
        static_fields: RefCell::new(static_fields),
        static_methods: Default::default(),
        constructor: None,
    };
    env.borrow_mut()
        .define("Saule".to_string(), Value::Class(Rc::new(class)));
}

/// `Saule.atLeast("26.7")` — is this toolchain that version or newer?
fn saule_at_least(args: &[Value]) -> Result<Vec<Value>, String> {
    let required = match args.first() {
        Some(Value::Str(s)) => (**s).clone(),
        Some(other) => {
            return Err(format!(
                "Saule.atLeast expects a string at argument 0, got `{}`",
                other.type_name()
            ));
        }
        None => return Err("Saule.atLeast expects a version string".to_string()),
    };
    Ok(vec![Value::Bool(saule_version::at_least(&required))])
}

fn native(name: &'static str, func: fn(&[Value]) -> Result<Vec<Value>, String>) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        // The fn pointer already implements `Fn`, so it boxes directly — the
        // wrapping closure the other stdlib modules use is redundant here.
        func: Box::new(func),
        param_names: Vec::new(),
    }))
}

/// Register native signatures for the typechecker.
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, register_const, t_named};

    let s = || t_named("string");
    let i = || t_named("integer");
    let b = || t_named("boolean");

    // `install` defines all of these unconditionally — the version is baked
    // in at compile time, so there is no configuration under which one of
    // them is absent.
    register_const("Saule.version", s());
    register_const("Saule.full", s());
    register_const("Saule.year", i());
    register_const("Saule.build", i());
    register_const("Saule.isDev", b());
    register_const("Saule.commit", s());

    register("Saule.atLeast", vec![s()], vec![b()]);
}
