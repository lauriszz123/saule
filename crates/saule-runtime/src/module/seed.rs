//! Collecting the names an import brings into scope, without
//! executing the module. Used by the checker and the LSP so a
//! not-yet-run import still resolves.

use saule_ast::{Decl, ImportNames, Module, Stmt};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::*;

/// Walk every `import ... from "path"` statement in `module`, resolve the
/// target file, parse it, and harvest its exported class / interface /
/// enum metadata into a [`saule_semantic::ModuleSeed`]. Returned to the
/// caller so they can hand it to [`saule_semantic::analyze_with_seed`] —
/// the result lets the typechecker know the return types of imported
/// methods like `Json.decode(...)`.
///
/// Best-effort: any import that fails to resolve, read, or parse is
/// silently skipped — semantic/typeck will surface the user-facing error
/// (or, in the import-fails case, the program driver will).
pub fn collect_import_seed(module: &Module, dir: &Path) -> saule_semantic::ModuleSeed {
    collect_import_seed_with(module, dir, &|_| None)
}

/// Where an imported module's source text comes from.
///
/// Returning `Some(text)` supplies that file's contents in place of what is
/// on disk; `None` means "read the file normally".
///
/// This exists for the language server. Its authoritative copy of an open
/// file is the buffer the editor last sent, which for an unsaved file is *not*
/// what `read_to_string` returns — so without an overlay, `main.sau`'s
/// diagnostics, hover, and completions were all computed against a stale
/// on-disk version of every file it imports. Editing a module and watching its
/// importer's errors fail to update was the visible symptom.
pub type SourceOverlay<'a> = &'a dyn Fn(&Path) -> Option<String>;

/// Where an imported module's parse *tree* comes from.
///
/// Returning `Some(tree)` skips reading and parsing that file entirely;
/// `None` means "do it yourself".
///
/// This exists because the walk is the same walk for every file in a
/// project, and the modules near the root of the import graph are therefore
/// parsed once per importer. A caller that already keeps parse trees — the
/// language server, `saule check` — hands them over instead, and the
/// duplicated work disappears. Only a *clean* tree may be supplied: the
/// walk's own parse is strict and skips a module it cannot parse, and a
/// recovered tree would smuggle declarations out of a file that does not
/// compile.
pub type ModuleSource<'a> = &'a dyn Fn(&Path) -> Option<std::sync::Arc<Module>>;

/// Where the walk gets what it needs to read a module.
#[derive(Clone, Copy)]
pub struct SeedIo<'a> {
    /// Consulted before the filesystem for a module's text.
    pub overlay: SourceOverlay<'a>,
    /// Consulted before either, for a module's tree.
    pub modules: Option<ModuleSource<'a>>,
}

impl<'a> SeedIo<'a> {
    /// Text from `overlay` or the filesystem; nothing pre-parsed.
    pub fn from_overlay(overlay: SourceOverlay<'a>) -> SeedIo<'a> {
        SeedIo {
            overlay,
            modules: None,
        }
    }
}

/// [`collect_import_seed`], but consulting `overlay` before the filesystem for
/// every module it reads — transitively, including through re-exports.
pub fn collect_import_seed_with(
    module: &Module,
    dir: &Path,
    overlay: SourceOverlay<'_>,
) -> saule_semantic::ModuleSeed {
    collect_import_seed_io(module, dir, SeedIo::from_overlay(overlay))
}

/// [`collect_import_seed_with`], with the option of being handed trees the
/// caller has already parsed. See [`ModuleSource`].
pub fn collect_import_seed_io(
    module: &Module,
    dir: &Path,
    io: SeedIo<'_>,
) -> saule_semantic::ModuleSeed {
    let mut visited = HashSet::new();
    let mut seed = collect_import_seed_inner(module, dir, &mut visited, 0, io);
    seed.wildcard_names = collect_wildcard_names(module, dir, io);
    seed
}

/// A module's tree: from the caller's cache if it has one, otherwise read
/// (through the overlay) and parsed here.
fn parsed_module(abs: &Path, io: SeedIo<'_>) -> Option<std::sync::Arc<Module>> {
    if let Some(provider) = io.modules
        && let Some(tree) = provider(abs)
    {
        return Some(tree);
    }
    let source = match (io.overlay)(abs) {
        Some(text) => text,
        None => std::fs::read_to_string(abs).ok()?,
    };
    let tokens = saule_lexer::Lexer::new(&source).tokenize().ok()?;
    saule_parser::parse(tokens).ok().map(std::sync::Arc::new)
}

/// Union of the local names every `from "..." import *` in `module` binds.
///
/// `None` when at least one wildcard target couldn't be enumerated — the
/// name resolver reads that as "unknown names may be in scope here" and
/// stops reporting undefined names for the whole module. Enumerating them
/// is what lets a typo inside a file that globs a module still be caught.
pub(crate) fn collect_wildcard_names(
    module: &Module,
    dir: &Path,
    io: SeedIo<'_>,
) -> Option<HashSet<String>> {
    let mut out = HashSet::new();
    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import {
            names: names @ (ImportNames::All | ImportNames::Namespace(_)),
            path,
            ..
        } = &d.value
        else {
            continue;
        };
        out.extend(static_import_names(dir, path, names, 0, io)?);
    }
    Some(out)
}

/// The local names one `import` statement binds, resolved statically —
/// from a package manifest or by parsing the target file, never by
/// executing anything.
///
/// `None` means the target couldn't be enumerated: an unresolvable path,
/// an unreadable or unparseable file, or a chain of re-exports nested deeper
/// than [`MAX_REEXPORT_DEPTH`].
pub(crate) fn static_import_names(
    dir: &Path,
    path: &str,
    names: &ImportNames,
    depth: usize,
    io: SeedIo<'_>,
) -> Option<Vec<String>> {
    // A name list is its own answer — no need to look at the target.
    if let ImportNames::List(_) = names {
        return Some(names.bind([]).into_iter().map(|(_, local)| local).collect());
    }
    let exports = static_export_names(dir, path, depth, io)?;
    Some(
        names
            .bind(exports.iter().map(String::as_str))
            .into_iter()
            .map(|(_, local)| local)
            .collect(),
    )
}

/// Every name the module at `path` exports: its own `export`ed declarations
/// and whatever its `export from … import …` statements pass on.
pub(crate) fn static_export_names(
    dir: &Path,
    path: &str,
    depth: usize,
    io: SeedIo<'_>,
) -> Option<Vec<String>> {
    if let Some(pkg) = crate::native_packages::lookup(path) {
        return Some(pkg.exports.iter().map(|n| (*n).to_string()).collect());
    }
    if crate::dynamic_packages::is_dynamic_package(path) {
        return Some(crate::dynamic_packages::export_names(path));
    }
    let abs = resolve_import_path(dir, path)?;
    let imported = parsed_module(&abs, io)?;
    let mut out: Vec<String> = imported
        .stmts
        .iter()
        .filter_map(|s| match &s.value {
            Stmt::Decl(d) => exported_name(&d.value).map(str::to_string),
            _ => None,
        })
        .collect();
    // The depth bound doubles as the cycle guard: a module that
    // (transitively) re-exports itself bottoms out here and reports "can't
    // enumerate" rather than recursing forever.
    let sub_dir = abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    for stmt in &imported.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import {
            names,
            path,
            exported: true,
            ..
        } = &d.value
        else {
            continue;
        };
        if depth >= MAX_REEXPORT_DEPTH {
            return None;
        }
        out.extend(static_import_names(&sub_dir, path, names, depth + 1, io)?);
    }
    Some(out)
}

/// How many nested re-exports we will follow when gathering type
/// metadata. Deep enough for any sane module tree, bounded so a pathological
/// (or cyclic) layout can't hang the typechecker.
pub(crate) const MAX_REEXPORT_DEPTH: usize = 8;

pub(crate) fn collect_import_seed_inner(
    module: &Module,
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
    io: SeedIo<'_>,
) -> saule_semantic::ModuleSeed {
    collect_seed(module, dir, visited, depth, io, false)
}

/// What `module`'s imports bring into scope, or with `only_exported` what its
/// `export from … import …` statements pass on to whoever imports it.
fn collect_seed(
    module: &Module,
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
    io: SeedIo<'_>,
    only_exported: bool,
) -> saule_semantic::ModuleSeed {
    let mut seed = saule_semantic::ModuleSeed::default();

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import {
            names,
            path,
            exported,
            ..
        } = &d.value
        else {
            continue;
        };
        if only_exported && !exported {
            continue;
        }

        // Native package seed — fold the package's synthetic
        // class/interface/enum metadata directly into the seed; no need
        // to parse anything from disk.
        if let Some(pkg) = crate::native_packages::lookup(path) {
            let built = (pkg.builtins)();
            let aliases = collect_native_aliases(pkg, names);
            for (orig, alias) in aliases {
                if let Some(info) = built.classes.get(&orig).cloned() {
                    seed.classes.entry(alias.clone()).or_insert(info);
                }
                if let Some(ext) = built.interfaces.get(&orig).cloned() {
                    seed.interfaces.entry(alias.clone()).or_insert(ext);
                }
                if let Some(info) = built.enums.get(&orig).cloned() {
                    seed.enums.entry(alias).or_insert(info);
                }
            }
            continue;
        }

        // Dynamic (shared-library) native package seed — synthesize a
        // semantic `ClassInfo` per exported class and an `EnumInfo` per enum,
        // so `Window.create(...)`, `img.width` and `BlendMode.Add` resolve.
        // Without this the loop below would try to read the synthetic
        // sentinel path as a file and silently skip the package, leaving
        // its types undefined.
        if crate::dynamic_packages::is_dynamic_package(path) {
            for (alias, info) in crate::dynamic_packages::seed_classes(path, names) {
                seed.classes.entry(alias).or_insert(info);
            }
            for (alias, info) in crate::dynamic_packages::seed_enums(path, names) {
                seed.enums.entry(alias).or_insert(info);
            }
            continue;
        }

        let Some(abs) = resolve_import_path(dir, path) else {
            continue;
        };
        let Some(imported) = parsed_module(&abs, io) else {
            continue;
        };

        let (mut reg, ifaces, enums, iface_methods, _iface_params) =
            saule_semantic::build_registry(&imported);
        let mut funcs = saule_semantic::build_function_registry(&imported);
        // An imported accessor's return type is inferred here, on the way
        // in. Doing it only in `analyze_inner` would infer it for the file
        // that declares the class and not for the files that use it, which
        // is the wrong way round: the declaring file can read the body.
        saule_semantic::infer_missing_returns(&imported, &mut reg, &ifaces, &enums, &mut funcs);
        let vars = saule_semantic::build_variable_registry(&imported);

        // For each top-level decl in the imported module, decide which
        // (local) alias to register it under. Wildcard imports adopt the
        // original name; named imports rename per `as`-clause.
        let aliases = collect_import_aliases(&imported, names);

        for (orig, alias) in aliases {
            if let Some(info) = reg.get(&orig).cloned() {
                seed.classes.entry(alias.clone()).or_insert(info);
            }
            if let Some(ext) = ifaces.get(&orig).cloned() {
                seed.interfaces.entry(alias.clone()).or_insert(ext);
            }
            // Carried beside `interfaces` so an *imported* interface's
            // method return types are as knowable as a local one's —
            // otherwise the fix would work only in single-file programs,
            // which is not where interfaces are used.
            if let Some(sigs) = iface_methods.get(&orig).cloned() {
                seed.interface_methods.entry(alias.clone()).or_insert(sigs);
            }
            if let Some(info) = enums.get(&orig).cloned() {
                seed.enums.entry(alias.clone()).or_insert(info);
            }
            if let Some(sig) = funcs.get(&orig).cloned() {
                seed.functions.entry(alias.clone()).or_insert(sig);
            }
            if let Some(ty) = vars.get(&orig).cloned() {
                seed.variables.entry(alias).or_insert(ty);
            }
        }

        // What the imported module passes on from its own imports lives one
        // level deeper. `visited` holds the modules on the current path, so a
        // cycle of re-exports stops instead of recursing.
        if depth < MAX_REEXPORT_DEPTH && visited.insert(abs.clone()) {
            let sub_dir = abs
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let sub = collect_seed(&imported, &sub_dir, visited, depth + 1, io, true);
            visited.remove(&abs);
            merge_reexported_seed(&mut seed, sub, names);
        }
    }

    seed
}

/// The names `key`, exported by the imported module, is known by here: a
/// namespace import binds it both bare and qualified.
fn local_names(names: &ImportNames, key: &str) -> Vec<String> {
    match names {
        ImportNames::All => vec![key.to_string()],
        ImportNames::Namespace(ns) => vec![format!("{ns}.{key}"), key.to_string()],
        ImportNames::List(items) => items
            .iter()
            .filter(|(orig, _)| orig == key)
            .map(|(orig, alias)| alias.clone().unwrap_or_else(|| orig.clone()))
            .collect(),
    }
}

fn merge_into<V: Clone>(
    to: &mut std::collections::HashMap<String, V>,
    from: std::collections::HashMap<String, V>,
    names: &ImportNames,
) {
    for (k, v) in from {
        for local in local_names(names, &k) {
            to.entry(local).or_insert_with(|| v.clone());
        }
    }
}

/// Fold what an imported module re-exports into `seed`, under the names this
/// import gives them.
pub(crate) fn merge_reexported_seed(
    seed: &mut saule_semantic::ModuleSeed,
    sub: saule_semantic::ModuleSeed,
    names: &ImportNames,
) {
    merge_into(&mut seed.classes, sub.classes, names);
    merge_into(&mut seed.interfaces, sub.interfaces, names);
    // Beside `interfaces` for the same reason the direct-import path carries
    // them: an interface whose method signatures were dropped registers as a
    // type, but every call through it resolves to an unknown return.
    merge_into(&mut seed.interface_methods, sub.interface_methods, names);
    merge_into(&mut seed.enums, sub.enums, names);
    merge_into(&mut seed.functions, sub.functions, names);
    merge_into(&mut seed.variables, sub.variables, names);
}

/// Resolve which `(original_name, local_alias)` pairs come into the
/// importing module's scope from one `import` statement.
pub(crate) fn collect_import_aliases(
    imported: &Module,
    names: &ImportNames,
) -> Vec<(String, String)> {
    let exports: Vec<&str> = imported
        .stmts
        .iter()
        .filter_map(|s| match &s.value {
            Stmt::Decl(d) => exported_name(&d.value),
            _ => None,
        })
        .collect();
    names.bind(exports)
}

/// Native-package counterpart of [`collect_import_aliases`]: wildcards
/// pull in every name the package declares via
/// [`crate::native_packages::NativePackage::exports`].
pub(crate) fn collect_native_aliases(
    pkg: &crate::native_packages::NativePackage,
    names: &ImportNames,
) -> Vec<(String, String)> {
    names.bind(pkg.exports.iter().copied())
}
