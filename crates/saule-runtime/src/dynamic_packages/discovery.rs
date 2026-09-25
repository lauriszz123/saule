//! Finding installed packages on disk and registering their
//! signatures with the typechecker.

// Only the `LIBS` cache below holds one, and that is feature-gated too.
#[cfg(feature = "native-packages")]
use libloading::Library;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once, RwLock};

use super::*;

/// Discovered packages keyed by import name. Populated once by [`discover`].
pub(crate) static MANIFESTS: RwLock<Option<HashMap<String, Arc<Manifest>>>> = RwLock::new(None);

/// Why an installed library is not a usable package, keyed by every name a
/// program might import it by. Consulted when an `import` does not resolve,
/// so the error says what is wrong with the package instead of claiming
/// there is none. Populated once by [`discover`].
static REJECTED: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

/// Loaded shared libraries, keyed by package name. Each library is kept
/// alive for the life of the process (the [`NativeClosure`]s built from it
/// hold raw function pointers into it).
#[cfg(feature = "native-packages")]
pub(crate) static LIBS: RwLock<Option<HashMap<String, Arc<Library>>>> = RwLock::new(None);

pub(crate) static DISCOVER_ONCE: Once = Once::new();

// ─── Filesystem layout ──────────────────────────────────────────────────────

/// The Saule home directory — the root of everything the toolchain installs
/// per-user: native packages, and (in future) the LSP server, docs, editor
/// plugins and the SDK/API surface.
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

/// Where packages are installed: one library file per package, which is the
/// whole package — its description is compiled into it.
pub fn packages_dir() -> PathBuf {
    saule_home().join("native_packages")
}

/// Where packages used to keep a separate TOML manifest. Nothing reads a
/// manifest from here any more; the directory is only looked at to explain
/// why a package installed the old way no longer imports.
fn legacy_manifests_dir() -> PathBuf {
    saule_home().join("native_manifests")
}

/// The extension a loadable library has on this platform.
pub(crate) fn library_extension() -> &'static str {
    if cfg!(windows) {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

// ─── Discovery ──────────────────────────────────────────────────────────────

/// Scan the packages directory and record every package found there.
/// Idempotent — only the first call does work.
///
/// Reading a package means reading its file, never loading it: the
/// metadata is parsed out of the library as data (see [`super::embedded`]).
/// A library that cannot be used is not an error here — a program that
/// never imports it should not hear about it — so the reason is recorded
/// instead, and reported by the `import` that names it.
pub fn discover() {
    DISCOVER_ONCE.call_once(|| {
        let (found, rejected) = scan(&packages_dir(), &legacy_manifests_dir());
        *MANIFESTS
            .write()
            .expect("dynamic manifest registry poisoned") = Some(found);
        *REJECTED.write().expect("rejection registry poisoned") = Some(rejected);
    });
}

type Scan = (HashMap<String, Arc<Manifest>>, HashMap<String, String>);

/// Read every library in `dir`. Split out of [`discover`] so it can be
/// tested against a directory of its own.
pub(crate) fn scan(dir: &Path, legacy_dir: &Path) -> Scan {
    let mut found: HashMap<String, Arc<Manifest>> = HashMap::new();
    let mut rejected: HashMap<String, String> = HashMap::new();
    // Import name → the files that declare it, to catch two installs of one
    // package.
    let mut claimed: HashMap<String, Vec<PathBuf>> = HashMap::new();

    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(library_extension()))
        .collect();
    // Directory order is arbitrary; sorting makes which duplicate is named
    // first — and every message — stable.
    files.sort();

    for path in files {
        match read_package(&path) {
            Ok(manifest) => {
                claimed
                    .entry(manifest.name.clone())
                    .or_default()
                    .push(path.clone());
                found.insert(manifest.name.clone(), Arc::new(manifest));
            }
            Err((reason, declared)) => {
                let text = rejection_text(&path, &reason);
                // Under the name the package declares, when that much was
                // readable — it is what a program imports — and under the
                // file's own names as well.
                if let Some(name) = declared {
                    rejected.insert(name, text.clone());
                }
                for key in names_for_file(&path) {
                    rejected.entry(key).or_insert_with(|| text.clone());
                }
            }
        }
    }

    // Two libraries claiming one import name: neither is chosen, since
    // picking one would make a program's behaviour depend on directory order.
    for (name, paths) in claimed {
        if paths.len() > 1 {
            found.remove(&name);
            let listed: Vec<String> = paths.iter().map(|p| format!("`{}`", p.display())).collect();
            rejected.insert(
                name.clone(),
                format!(
                    "native package `{name}` is installed more than once ({}); \
                     remove all but one",
                    listed.join(", ")
                ),
            );
        }
    }

    // Packages installed the old way: a TOML manifest in its own directory.
    // Only worth a word if the name is otherwise unaccounted for.
    for entry in std::fs::read_dir(legacy_dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let name = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| t.parse::<toml::Table>().ok())
            .and_then(|t| {
                t.get("package")?
                    .get("name")?
                    .as_str()
                    .map(str::to_string)
            });
        let Some(name) = name else { continue };
        if found.contains_key(&name) || rejected.contains_key(&name) {
            continue;
        }
        rejected.insert(
            name.clone(),
            format!(
                "native package `{name}` is installed the old way, as a separate \
                 manifest (`{}`) beside its library. A package now carries its \
                 description inside the library: rebuild it with the current \
                 `saule-sdk`, copy just the library into `{}`, and delete `{}`",
                path.display(),
                dir.display(),
                legacy_dir.display(),
            ),
        );
    }

    (found, rejected)
}

/// Read one library as a package. On failure, the reason and — if the
/// package record was readable — the import name it declares.
fn read_package(path: &Path) -> Result<Manifest, (String, Option<String>)> {
    let bytes = std::fs::read(path).map_err(|e| (format!("could not be read ({e})"), None))?;
    let records = embedded::read_records(&bytes).map_err(|e| (e, None))?;
    if records.is_empty() {
        return Err((
            "has no Saule metadata in it: it is not a Saule native package, or it \
             was built with an SDK from before packages carried their own description"
                .to_string(),
            None,
        ));
    }
    manifest_from_records(&records, path).map_err(|e| (e, declared_name(&records)))
}

/// The import name a set of records declares, if its package record can be
/// read at all.
fn declared_name(records: &[embedded::RawRecord]) -> Option<String> {
    records
        .iter()
        .filter(|r| r.symbol == "PACKAGE")
        .find_map(|r| r.payload.parse::<toml::Table>().ok())
        .and_then(|t| t.get("name")?.as_str().map(str::to_string))
}

/// The names a program might use for a library whose metadata could not be
/// read: its file stem, with and without the `lib` prefix Unix toolchains
/// add (`libsaule_engine_lib.so` → `saule_engine_lib`).
fn names_for_file(path: &Path) -> Vec<String> {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    let mut names = vec![stem.to_string()];
    if let Some(bare) = stem.strip_prefix("lib") {
        names.push(bare.to_string());
    }
    names
}

fn rejection_text(path: &Path, reason: &str) -> String {
    format!("the native package in `{}` {reason}", path.display())
}

/// Why `name` names an installed library that could not be used as a
/// package, if it does.
pub fn rejection(name: &str) -> Option<String> {
    discover();
    REJECTED
        .read()
        .expect("rejection registry poisoned")
        .as_ref()
        .and_then(|m| m.get(name).cloned())
}

/// Register every discovered package's static member signatures with
/// `saule-typeck`. Wired into the typeck initializer so it runs once per
/// thread that type-checks — see [`crate::stdlib::register_all_sigs`].
///
/// Only *static* members go into this table: it is keyed `Class.member`,
/// and a class listed in it counts as a module whose members may be called
/// statically. An object's methods and properties are typed through the
/// class metadata [`seed_classes`] gives the checker instead, the same way
/// a Saule class's are, which is what keeps `Image.width()` — an instance
/// method called on the class — an error.
pub fn register_sigs() {
    let guard = MANIFESTS
        .read()
        .expect("dynamic manifest registry poisoned");
    let Some(map) = guard.as_ref() else { return };
    for manifest in map.values() {
        for class in &manifest.exports {
            for method in class.members(Receiver::Static) {
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

/// The names `name` exports — its classes, then its enums, each sorted.
/// Empty if `name` isn't a discovered dynamic package. Used by tooling (the
/// LSP's import hover) that wants the package's surface without loading its
/// shared library.
pub fn export_names(name: &str) -> Vec<String> {
    lookup(name)
        .map(|m| {
            m.exports
                .iter()
                .map(|c| c.name.clone())
                .chain(m.enums.iter().map(|e| e.name.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Every doc comment package `name` carries, as `(qualified name, text)`
/// pairs keyed the way the editor's doc index keys a Saule module's: `Class`,
/// `Class.member`, `Enum`, `Enum.Variant`. The text is the author's `///`
/// comment, Markdown as written. Empty for an unknown package.
pub fn package_docs(name: &str) -> Vec<(String, String)> {
    let Some(m) = lookup(name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for class in &m.exports {
        if let Some(d) = &class.doc {
            out.push((class.name.clone(), d.clone()));
        }
        for member in &class.methods {
            if let Some(d) = &member.doc {
                out.push((format!("{}.{}", class.name, member.name), d.clone()));
            }
        }
    }
    for e in &m.enums {
        if let Some(d) = &e.doc {
            out.push((e.name.clone(), d.clone()));
        }
        for (variant, d) in e.variants.iter().zip(&e.variant_docs) {
            if !d.is_empty() {
                out.push((format!("{}.{variant}", e.name), d.clone()));
            }
        }
    }
    out
}

// ─── Import-resolution integration ──────────────────────────────────────────

/// Mint the sentinel `PathBuf` an `import` of a dynamic package resolves
/// to. Mirrors [`crate::native_packages::sentinel_path`] but with a
/// distinct prefix so the two kinds never collide.
pub fn sentinel_path(name: &str) -> PathBuf {
    PathBuf::from(format!("__saule_dynamic__/{name}"))
}

/// Inverse of [`sentinel_path`]; `None` for any non-dynamic path.
pub fn name_from_sentinel(path: &Path) -> Option<&str> {
    path.to_str()?.strip_prefix("__saule_dynamic__/")
}
