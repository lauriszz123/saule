//! Standard library wiring.
//!
//! The stdlib is shipped as a set of [`crate::native_packages::NativePackage`]s.
//! Each module here exposes a `pub static *_PACKAGE: NativePackage` that
//! describes its bindings, type signatures, and auto-prelude policy.
//! Registering them with `native_packages::register` is the only thing
//! [`register_builtin_packages`] does.
//!
//! Embedders get four orthogonal hooks the rest of the interpreter
//! drives off of:
//!
//! * [`crate::native_packages::install_auto_prelude`] — fired by
//!   [`crate::env::Environment::with_prelude`] via [`install_std`].
//! * [`saule_typeck::sigs::set_initializer`] — [`register_all_sigs`]
//!   walks every registered package's `register_sigs`.
//! * [`saule_semantic::builtins::set_provider`] —
//!   [`builtin_registries`] returns the aggregated builtins.
//! * [`saule_semantic::prelude::set_provider`] — [`all_prelude_names`]
//!   returns the aggregated auto-prelude names.

use std::cell::RefCell;
use std::rc::Rc;

use crate::env::Environment;
use crate::value::{NativeFn, Value};

pub mod core;
pub mod io;
pub mod iter;
pub mod math;
pub mod ops;
pub mod os;
pub mod project;
pub mod sigs;
pub mod string;
pub mod table;
pub mod version;

/// Install every `auto_prelude` package's bindings into `env`. Called
/// by [`crate::env::Environment::with_prelude`] right after the env is
/// allocated. Third-party packages registered via
/// [`crate::native_packages::register`] also flow through here.
pub fn install_std(env: &Rc<RefCell<Environment>>) {
    crate::native_packages::install_auto_prelude(env);
}

/// Register every built-in native package. Called from [`crate::init`]
/// after the typeck/semantic providers are installed. Each package's
/// `register_sigs` and `builtins` thunks fire on registration so the
/// typechecker sees them right away; the `exports` list is folded into
/// the prelude-name aggregator if `auto_prelude` is on.
pub fn register_builtin_packages() {
    use crate::native_packages::register;
    register(&core::CORE_PACKAGE);
    register(&iter::ITER_PACKAGE);
    register(&ops::OPS_PACKAGE);
    register(&math::MATH_PACKAGE);
    register(&string::STRING_PACKAGE);
    register(&table::TABLE_PACKAGE);
    register(&io::IO_PACKAGE);
    register(&os::OS_PACKAGE);
    register(&project::PROJECT_PACKAGE);
    register(&version::VERSION_PACKAGE);
}

/// Walk every registered package and let it register its native
/// signatures with `saule-typeck`. Wired into the lazy initializer in
/// [`crate::init`] so typecheck-only passes see them without needing
/// the runtime environment to have been built first.
pub fn register_all_sigs() {
    for pkg in crate::native_packages::all() {
        (pkg.register_sigs)();
    }
    // Dynamically-discovered native packages register their signatures from
    // the parsed manifests so `Pkg.method(...)` type-checks before the
    // shared library is ever loaded.
    crate::dynamic_packages::register_sigs();
}

/// Synthetic `ClassInfo` / `EnumInfo` entries for stdlib value types
/// (e.g. `FsInfo`, `FsKind`) contributed by registered packages.
/// Installed via [`saule_semantic::builtins::set_provider`] in
/// [`crate::init`].
pub fn builtin_registries() -> saule_semantic::builtins::Builtins {
    crate::native_packages::aggregated_builtins()
}

/// Every identifier registered `auto_prelude` packages inject into the
/// global prelude. Consumed by `saule-semantic`'s name resolver so
/// references like `print`, `Math`, `Iterable`, … aren't flagged as
/// undefined.
pub fn all_prelude_names() -> Vec<&'static str> {
    crate::native_packages::aggregated_prelude()
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
