//! Native-package registry — the single extension point through which any
//! out-of-core capability (filesystem, networking, graphics, audio, …)
//! plugs into the language.
//!
//! A [`NativePackage`] is a `&'static` descriptor that bundles:
//!
//! * a unique `name` — the literal users write in `import { ... } from "X"`,
//! * an `install` thunk that defines the package's bindings into an
//!   `Environment` — same shape the legacy stdlib `install` fns already
//!   use, so existing code converts in a few lines,
//! * an `exports` slice listing which names the loader should harvest
//!   from the scratch env to expose via `import`,
//! * a `register_sigs` thunk fired once at [`register`]-time so the
//!   typechecker sees the package's static-method signatures,
//! * a `builtins` thunk producing the synthetic `ClassInfo` / `EnumInfo`
//!   the typechecker needs for value types the package exposes (e.g.
//!   `File`, `FsInfo`),
//! * an `auto_prelude` flag — when `true`, the package's `install` is
//!   also fired directly against the global prelude on environment
//!   construction so bare-name references keep working. Set to `false`
//!   for any package that should require explicit import (the common
//!   case for third-party packages).
//!
//! Packages live anywhere — in this crate, in a sibling crate, or in a
//! dylib loaded at runtime — as long as someone calls [`register`] with
//! a `&'static NativePackage` before the first import that names it.
//!
//! ## Lifecycle
//!
//! 1. Embedder builds (or links in) a package and calls [`register`] at
//!    startup. We immediately fire `register_sigs` and stash `builtins`
//!    in a thread-local aggregator consumed by the semantic provider.
//! 2. User code says `import { X } from "name"`. The module loader
//!    checks [`lookup`]; on a hit it skips filesystem resolution, runs
//!    `install` against a scratch env, harvests `exports` into a
//!    [`ModuleExports`], and caches it.
//! 3. The bound names land in the importing module's scope exactly like
//!    they would for a file-based module.
//!
//! ## Sentinel path
//!
//! The module cache is keyed by `PathBuf`. Native packages don't have a
//! real path, so we mint one — `__saule_native__/<name>` — via
//! [`sentinel_path`]. Cache hits and the `loading` circular-import set
//! work identically for native and file-based modules.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::RwLock;

use crate::env::Environment;
use crate::module::ModuleExports;

/// Static descriptor of a native package. Constructible in a `static`
/// item so packages can be defined as `pub static FOO: NativePackage = ...`.
pub struct NativePackage {
    /// Import name. Matches the literal in `import ... from "<name>"`.
    pub name: &'static str,

    /// Package version, semver. Currently informational; reserved for a
    /// future `min_core` / dependency-resolution pass.
    pub version: &'static str,

    /// Define every binding the package contributes into `env`. Same
    /// shape as the historical stdlib `install` fns — call
    /// `env.borrow_mut().define(name, value)` for each export. Invoked
    /// once per scope: once against the prelude on auto-prelude
    /// installation, and once per first-time `import` against a scratch
    /// env (whose contents are harvested via [`Self::exports`]).
    pub install: fn(&Rc<RefCell<Environment>>),

    /// Names produced by [`Self::install`] that should be exposed via
    /// `import`. The loader harvests exactly these out of the scratch
    /// env it passes to `install`. Wildcards (`import * from "x"`)
    /// pull in every entry.
    pub exports: &'static [&'static str],

    /// Register native function / method signatures with `saule-typeck`.
    /// Fired once at [`register`]-time.
    pub register_sigs: fn(),

    /// Provide synthetic class / interface / enum metadata for value
    /// types the package exposes. Folded into the semantic builtin
    /// registry at [`register`]-time.
    pub builtins: fn() -> saule_semantic::builtins::Builtins,

    /// When `true`, [`Self::install`] is also fired directly against
    /// the global prelude on [`crate::Environment::with_prelude`] so
    /// bare references like `Io.open(...)` work without an explicit
    /// import. Defaults to `false` for new third-party packages — make
    /// explicit import the norm.
    pub auto_prelude: bool,
}

// Registries are process-global (not thread-local) so a multi-threaded
// test harness or embedder sees the same set of packages from every
// thread after a single `register` call. `&'static NativePackage` is
// `Send + Sync`, and `Builtins` is owned data we clone on read.

/// Process-global registry of every package that has been installed.
static REGISTRY: RwLock<Option<HashMap<&'static str, &'static NativePackage>>> = RwLock::new(None);

/// Registration order — drives auto-prelude install order so a
/// package's `install` runs after any package it implicitly relies on
/// (e.g. `os` after `iter` would need this if `Os` ever returned an
/// `Iterable`; today the order is irrelevant but it costs nothing to
/// make it deterministic).
static ORDER: RwLock<Option<Vec<&'static str>>> = RwLock::new(None);

/// Aggregated builtins from every registered package. Consumed by the
/// semantic builtins provider; appended to on every [`register`].
static AGGREGATED_BUILTINS: RwLock<Option<saule_semantic::builtins::Builtins>> = RwLock::new(None);

/// Aggregated prelude names from every `auto_prelude` package.
static AGGREGATED_PRELUDE: RwLock<Option<Vec<&'static str>>> = RwLock::new(None);

fn with_registry_mut<R>(
    f: impl FnOnce(&mut HashMap<&'static str, &'static NativePackage>) -> R,
) -> R {
    let mut guard = REGISTRY.write().expect("native package registry poisoned");
    f(guard.get_or_insert_with(HashMap::new))
}

fn with_order_mut<R>(f: impl FnOnce(&mut Vec<&'static str>) -> R) -> R {
    let mut guard = ORDER.write().expect("native package order poisoned");
    f(guard.get_or_insert_with(Vec::new))
}

fn with_builtins_mut<R>(f: impl FnOnce(&mut saule_semantic::builtins::Builtins) -> R) -> R {
    let mut guard = AGGREGATED_BUILTINS
        .write()
        .expect("native package builtins poisoned");
    f(guard.get_or_insert_with(saule_semantic::builtins::Builtins::default))
}

fn with_prelude_mut<R>(f: impl FnOnce(&mut Vec<&'static str>) -> R) -> R {
    let mut guard = AGGREGATED_PRELUDE
        .write()
        .expect("native package prelude poisoned");
    f(guard.get_or_insert_with(Vec::new))
}

/// Register `pkg`. Idempotent: a second call with the same `name` is a
/// no-op. Fires `register_sigs` and folds `builtins` into the global
/// aggregator immediately so the typechecker sees the package even
/// before its first import. If `auto_prelude` is `true`, the package's
/// `exports` are also added to the resolver's prelude name set.
pub fn register(pkg: &'static NativePackage) {
    let inserted = with_registry_mut(|map| {
        if map.contains_key(pkg.name) {
            false
        } else {
            map.insert(pkg.name, pkg);
            true
        }
    });
    if !inserted {
        return;
    }
    with_order_mut(|o| o.push(pkg.name));
    (pkg.register_sigs)();
    let built = (pkg.builtins)();
    with_builtins_mut(|agg| {
        for (k, v) in built.classes {
            agg.classes.entry(k).or_insert(v);
        }
        for (k, v) in built.interfaces {
            agg.interfaces.entry(k).or_insert(v);
        }
        for (k, v) in built.enums {
            agg.enums.entry(k).or_insert(v);
        }
    });
    if pkg.auto_prelude {
        with_prelude_mut(|p| p.extend(pkg.exports.iter().copied()));
    }
}

/// Look up a registered package by import name.
pub fn lookup(name: &str) -> Option<&'static NativePackage> {
    let guard = REGISTRY.read().expect("native package registry poisoned");
    guard.as_ref().and_then(|m| m.get(name).copied())
}

/// Snapshot of every package currently registered, in registration order.
pub fn all() -> Vec<&'static NativePackage> {
    let order = ORDER.read().expect("native package order poisoned");
    let map = REGISTRY.read().expect("native package registry poisoned");
    match (order.as_ref(), map.as_ref()) {
        (Some(o), Some(m)) => o.iter().filter_map(|n| m.get(n).copied()).collect(),
        _ => Vec::new(),
    }
}

/// Snapshot of the aggregated builtins from every registered package.
/// Plugged into `saule_semantic::builtins::set_provider` by
/// [`crate::init`].
pub fn aggregated_builtins() -> saule_semantic::builtins::Builtins {
    let guard = AGGREGATED_BUILTINS
        .read()
        .expect("native package builtins poisoned");
    guard.clone().unwrap_or_default()
}

/// Snapshot of the aggregated prelude names contributed by
/// `auto_prelude` packages.
pub fn aggregated_prelude() -> Vec<&'static str> {
    let guard = AGGREGATED_PRELUDE
        .read()
        .expect("native package prelude poisoned");
    guard.clone().unwrap_or_default()
}

/// Mint the sentinel `PathBuf` used as the module cache key for `name`.
/// Not a real filesystem path — never opened, only compared.
pub fn sentinel_path(name: &str) -> PathBuf {
    PathBuf::from(format!("__saule_native__/{name}"))
}

/// Inverse of [`sentinel_path`] — extract the package name from a
/// sentinel `PathBuf`, returning `None` for a real filesystem path.
pub fn name_from_sentinel(path: &Path) -> Option<&str> {
    let s = path.to_str()?;
    s.strip_prefix("__saule_native__/")
}

/// Build a [`ModuleExports`] for `pkg` by running its `install` against
/// a scratch environment and harvesting [`NativePackage::exports`].
/// Called by the module loader on first `import` of `pkg`.
pub fn build_exports(pkg: &NativePackage) -> ModuleExports {
    let env = Environment::new();
    (pkg.install)(&env);
    let mut exports = ModuleExports::default();
    let e = env.borrow();
    for name in pkg.exports {
        if let Some(value) = e.get(name) {
            exports.values.insert((*name).to_string(), value);
        }
    }
    exports
}

/// Install every `auto_prelude` package's bindings into `env`. Called
/// by [`crate::Environment::with_prelude`] right after the core stdlib.
pub fn install_auto_prelude(env: &Rc<RefCell<Environment>>) {
    for pkg in all() {
        if pkg.auto_prelude {
            (pkg.install)(env);
        }
    }
}
