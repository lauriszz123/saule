//! Prelude-name registry consumed by [`crate::resolve`].
//!
//! The name-resolution pass needs to know which identifiers the embedder's
//! standard library injects into the top-level environment (so referencing
//! `print`, `Math`, `Iterable`, etc. isn't reported as undefined). Since
//! this crate doesn't link the stdlib, embedders install a hook via
//! [`set_provider`] that returns the full list lazily on first use.
//!
//! The provider slot is process-global so a multi-threaded test harness
//! only has to install it once; the materialised name set is then cached
//! per-thread via a thread-local on first lookup.
//!
//! If no provider is installed, the registry is empty and the resolver
//! treats every otherwise-unknown bare ident as undefined — which is what
//! you want in a stand-alone "language only" build.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::OnceLock;

type Provider = fn() -> Vec<&'static str>;

/// Process-global provider slot. Set once by the embedder.
static PROVIDER: OnceLock<Provider> = OnceLock::new();

thread_local! {
    static NAMES: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static INIT_DONE: Cell<bool> = const { Cell::new(false) };
}

/// Install the function the embedder wants to run lazily on first
/// resolver lookup. Typically called once at process startup by the
/// interpreter so its stdlib's global names appear in the registry.
/// Subsequent calls are silently ignored (`OnceLock` semantics).
pub fn set_provider(f: Provider) {
    let _ = PROVIDER.set(f);
}

/// Borrow the prelude name set. Triggers a one-shot run of the embedder's
/// provider on first access per thread.
pub fn with_names<R>(f: impl FnOnce(&HashSet<String>) -> R) -> R {
    ensure_loaded();
    NAMES.with(|n| f(&n.borrow()))
}

/// Is `name` a known prelude global?
pub fn contains(name: &str) -> bool {
    with_names(|s| s.contains(name))
}

fn ensure_loaded() {
    if INIT_DONE.with(Cell::get) {
        return;
    }
    INIT_DONE.with(|d| d.set(true));
    if let Some(provider) = PROVIDER.get() {
        let names = provider();
        NAMES.with(|n| {
            let mut set = n.borrow_mut();
            for s in names {
                set.insert(s.to_string());
            }
        });
    }
}

