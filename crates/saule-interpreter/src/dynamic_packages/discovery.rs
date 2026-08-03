//! Finding installed packages on disk and registering their
//! signatures with the typechecker.

use libloading::Library;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once, RwLock};

use super::*;

/// Discovered manifests keyed by import name. Populated once by [`discover`].
pub(crate) static MANIFESTS: RwLock<Option<HashMap<String, Arc<Manifest>>>> = RwLock::new(None);

/// Loaded shared libraries, keyed by package name. Each library is kept
/// alive for the life of the process (the [`NativeClosure`]s built from it
/// hold raw function pointers into it).
#[cfg(feature = "native-packages")]
pub(crate) static LIBS: RwLock<Option<HashMap<String, Arc<Library>>>> = RwLock::new(None);

pub(crate) static DISCOVER_ONCE: Once = Once::new();

// ─── Filesystem layout ──────────────────────────────────────────────────────

/// The Saule home directory — the root of everything the toolchain installs
/// per-user: native packages and their manifests, and (in future) the LSP
/// server, docs, editor plugins and the SDK/API surface.
///
/// `SAULE_HOME`, when set, **is** that directory — it is used verbatim, not
/// treated as a parent to append `.saule` to. This matches how the install
/// scripts (`scripts/install_*.sh`, `scripts/install_windows.ps1`) interpret
/// the variable. Unset, it defaults to `.saule` under the user's home.
pub(crate) fn saule_home() -> PathBuf {
    if let Some(explicit) = std::env::var_os("SAULE_HOME") {
        return PathBuf::from(explicit);
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".saule")
}

pub(crate) fn manifests_dir() -> PathBuf {
    saule_home().join("native_manifests")
}

#[cfg(feature = "native-packages")]
pub(crate) fn packages_dir() -> PathBuf {
    saule_home().join("native_packages")
}

// ─── Discovery ──────────────────────────────────────────────────────────────

/// Scan `~/.saule/native_manifests/` and record every valid manifest. Idempotent
/// — only the first call does work. A malformed manifest is logged to stderr
/// and skipped rather than aborting startup.
pub fn discover() {
    DISCOVER_ONCE.call_once(|| {
        let mut map = std::collections::HashMap::new();
        let dir = manifests_dir();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    continue;
                }
                match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|t| parse_manifest(&t))
                {
                    Ok(manifest) => {
                        map.insert(manifest.name.clone(), Arc::new(manifest));
                    }
                    Err(err) => {
                        eprintln!(
                            "saule: ignoring native manifest `{}`: {err}",
                            path.display()
                        );
                    }
                }
            }
        }
        *MANIFESTS
            .write()
            .expect("dynamic manifest registry poisoned") = Some(map);
    });
}

/// Register every discovered package's method signatures with `saule-typeck`.
/// Wired into the typeck initializer so it runs once per thread that
/// type-checks — see [`crate::stdlib::register_all_sigs`].
pub fn register_sigs() {
    let guard = MANIFESTS
        .read()
        .expect("dynamic manifest registry poisoned");
    let Some(map) = guard.as_ref() else { return };
    for manifest in map.values() {
        for class in &manifest.exports {
            for method in &class.methods {
                let qname = format!("{}.{}", class.name, method.name);
                if method.type_params.is_empty() {
                    saule_typeck::sigs::register(
                        &qname,
                        method.params.clone(),
                        method.returns.clone(),
                    );
                } else {
                    let tps: Vec<&str> = method.type_params.iter().map(String::as_str).collect();
                    saule_typeck::sigs::register_g(
                        &qname,
                        tps,
                        method.params.clone(),
                        method.returns.clone(),
                    );
                }
            }
        }
    }
}

/// Look up a discovered package by import name.
pub(crate) fn lookup(name: &str) -> Option<Arc<Manifest>> {
    let guard = MANIFESTS
        .read()
        .expect("dynamic manifest registry poisoned");
    guard.as_ref().and_then(|m| m.get(name).cloned())
}

/// Is `name` a discovered dynamic package?
pub fn is_dynamic_package(name: &str) -> bool {
    lookup(name).is_some()
}

/// Every discovered dynamic package's import name. Used by tooling (the LSP's
/// import completion) to offer installed packages as import targets.
pub fn package_names() -> Vec<String> {
    let guard = MANIFESTS
        .read()
        .expect("dynamic manifest registry poisoned");
    guard
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// The class names `name` exports, in manifest order. Empty if `name` isn't
/// a discovered dynamic package. Used by tooling (the LSP's import hover)
/// that wants the package's surface without loading its shared library.
pub fn export_names(name: &str) -> Vec<String> {
    lookup(name)
        .map(|m| m.exports.iter().map(|c| c.name.clone()).collect())
        .unwrap_or_default()
}

// ─── Module-loader integration ──────────────────────────────────────────────

/// Mint the sentinel `PathBuf` used as the module-cache key for a dynamic
/// package. Mirrors [`crate::native_packages::sentinel_path`] but with a
/// distinct prefix so the two kinds never collide.
pub fn sentinel_path(name: &str) -> PathBuf {
    PathBuf::from(format!("__saule_dynamic__/{name}"))
}

/// Inverse of [`sentinel_path`]; `None` for any non-dynamic path.
pub fn name_from_sentinel(path: &Path) -> Option<&str> {
    path.to_str()?.strip_prefix("__saule_dynamic__/")
}
