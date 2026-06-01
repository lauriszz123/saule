//! Builtin class / interface / enum registry consumed by
//! [`crate::analyze_with_seed`].
//!
//! The embedder (typically the interpreter's stdlib) installs a
//! [`Provider`] that returns synthetic [`ClassInfo`] / [`EnumInfo`]
//! entries for stdlib types whose declarations don't exist in user
//! source (e.g. `Os.FsInfo`, `Os.FsKind`). `analyze_with_seed` folds
//! these in *after* the seed but *before* user-declared types, so a
//! locally-declared `class FsInfo` still wins.
//!
//! Same shape as [`crate::prelude`]: process-global `OnceLock`
//! provider slot, lazy thread-local materialisation, silently empty
//! when no provider is installed.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use super::registry::{ClassRegistry, EnumRegistry, InterfaceRegistry};

/// Materialised builtin registries.
#[derive(Default, Clone, Debug)]
pub struct Builtins {
    pub classes: ClassRegistry,
    pub interfaces: InterfaceRegistry,
    pub enums: EnumRegistry,
}

type Provider = fn() -> Builtins;

static PROVIDER: OnceLock<Provider> = OnceLock::new();

thread_local! {
    static CACHE: RefCell<Builtins> = RefCell::new(Builtins::default());
    static INIT_DONE: Cell<bool> = const { Cell::new(false) };
}

/// Install the builtin-registry provider. Typically called once at
/// startup by the interpreter. Subsequent calls are silently ignored.
pub fn set_provider(f: Provider) {
    let _ = PROVIDER.set(f);
}

/// Return a clone of the materialised builtins. Triggers a one-shot
/// run of the provider on first access per thread.
pub fn snapshot() -> Builtins {
    ensure_loaded();
    CACHE.with(|c| c.borrow().clone())
}

fn ensure_loaded() {
    if INIT_DONE.with(Cell::get) {
        return;
    }
    INIT_DONE.with(|d| d.set(true));
    if let Some(provider) = PROVIDER.get() {
        let built = provider();
        CACHE.with(|c| *c.borrow_mut() = built);
    }
}
