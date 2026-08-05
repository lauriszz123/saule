//! Collecting the names an import brings into scope, without
//! executing the module. Used by the checker and the LSP so a
//! not-yet-run import still resolves.

use saule_ast::{Decl, ImportNames, Module, Stmt};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::*;

/// The local names one `import` statement binds into its module's scope.
///
/// For a glob the names come from the target's own exports. The module has
/// already executed this import by the time we run, so those exports are in
/// the loader cache — no need to re-resolve or re-run anything.
pub(crate) fn imported_local_names(
    names: &ImportNames,
    path: &str,
    dir: &Path,
    loader: &Rc<RefCell<ModuleLoader>>,
) -> Vec<String> {
    match names {
        ImportNames::List(items) => items
            .iter()
            .map(|(orig, alias)| alias.clone().unwrap_or_else(|| orig.clone()))
            .collect(),
        ImportNames::All => resolve_import_path(dir, path)
            .and_then(|abs| {
                loader
                    .borrow()
                    .cache
                    .get(&abs)
                    .map(|e| e.values.keys().cloned().collect::<Vec<_>>())
            })
            .unwrap_or_default(),
    }
}

/// Walk every `import ... from "path"` statement in `module`, resolve the
/// target file, parse it, and harvest its exported class / interface /
/// enum metadata into a [`saule_semantic::ModuleSeed`]. Returned to the
/// caller so they can hand it to [`saule_semantic::analyze_with_seed`] —
/// the result lets the typechecker know the return types of imported
/// methods like `Json.decode(...)`.
///
/// Best-effort: any import that fails to resolve, read, or parse is
/// silently skipped — semantic/typeck will surface the user-facing error
/// (or, in the import-fails case, the runtime loader will).
pub fn collect_import_seed(module: &Module, dir: &Path) -> saule_semantic::ModuleSeed {
    let mut visited = HashSet::new();
    let mut seed = collect_import_seed_inner(module, dir, &mut visited, 0);
    seed.wildcard_names = collect_wildcard_names(module, dir);
    seed
}

/// Union of the local names every `import * from "..."` in `module` binds.
///
/// `None` when at least one wildcard target couldn't be enumerated — the
/// name resolver reads that as "unknown names may be in scope here" and
/// stops reporting undefined names for the whole module. Enumerating them
/// is what lets a typo inside a file that globs a module still be caught.
pub(crate) fn collect_wildcard_names(module: &Module, dir: &Path) -> Option<HashSet<String>> {
    let mut out = HashSet::new();
    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import {
            names: names @ ImportNames::All,
            path,
            ..
        } = &d.value
        else {
            continue;
        };
        out.extend(static_import_names(dir, path, names, 0)?);
    }
    Some(out)
}

/// The local names one `import` statement binds, resolved statically —
/// from a package manifest or by parsing the target file, never by
/// executing anything.
///
/// `None` means the target couldn't be enumerated: an unresolvable path,
/// an unreadable or unparseable file, or a barrel chain nested deeper
/// than [`MAX_BARREL_DEPTH`].
pub(crate) fn static_import_names(
    dir: &Path,
    path: &str,
    names: &ImportNames,
    depth: usize,
) -> Option<Vec<String>> {
    // A name list is its own answer — no need to look at the target.
    if let ImportNames::List(items) = names {
        return Some(
            items
                .iter()
                .map(|(orig, alias)| alias.clone().unwrap_or_else(|| orig.clone()))
                .collect(),
        );
    }

    // Glob over a native package: the descriptor / manifest already lists
    // exactly what the import binds.
    if let Some(pkg) = crate::native_packages::lookup(path) {
        return Some(pkg.exports.iter().map(|n| (*n).to_string()).collect());
    }
    if crate::dynamic_packages::is_dynamic_package(path) {
        return Some(crate::dynamic_packages::export_names(path));
    }

    // Glob over a file module: its `export`ed top-level declarations.
    let abs = resolve_import_path(dir, path)?;
    let source = std::fs::read_to_string(&abs).ok()?;
    let tokens = saule_lexer::Lexer::new(&source).tokenize().ok()?;
    let imported = saule_parser::parse(tokens).ok()?;

    let mut out: Vec<String> = imported
        .stmts
        .iter()
        .filter_map(|s| match &s.value {
            Stmt::Decl(d) => exported_name(&d.value).map(str::to_string),
            _ => None,
        })
        .collect();

    // A barrel re-exports whatever *it* imports, so its surface is wider
    // than its own declarations. The depth bound doubles as the cycle
    // guard: a barrel that (transitively) globs itself bottoms out here
    // and reports "can't enumerate" rather than recursing forever.
    if is_init_module(&abs) {
        if depth >= MAX_BARREL_DEPTH {
            return None;
        }
        let sub_dir = abs
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        for stmt in &imported.stmts {
            let Stmt::Decl(d) = &stmt.value else { continue };
            let Decl::Import { names, path, .. } = &d.value else {
                continue;
            };
            out.extend(static_import_names(&sub_dir, path, names, depth + 1)?);
        }
    }

    Some(out)
}

/// How many nested `init.sau` barrels we will follow when gathering type
/// metadata. Deep enough for any sane module tree, bounded so a pathological
/// (or cyclic) layout can't hang the typechecker.
pub(crate) const MAX_BARREL_DEPTH: usize = 8;

pub(crate) fn collect_import_seed_inner(
    module: &Module,
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> saule_semantic::ModuleSeed {
    let mut seed = saule_semantic::ModuleSeed::default();

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import { names, path, .. } = &d.value else {
            continue;
        };

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

        // Dynamic (manifest-described) native package seed — synthesize a
        // semantic `ClassInfo` per exported class so member access like
        // `Window.create(...)` resolves. Without this the loop below would
        // try to read the synthetic sentinel path as a file and silently
        // skip the package, leaving its classes undefined.
        if crate::dynamic_packages::is_dynamic_package(path) {
            for (alias, info) in crate::dynamic_packages::seed_classes(path, names) {
                seed.classes.entry(alias).or_insert(info);
            }
            continue;
        }

        let Some(abs) = resolve_import_path(dir, path) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&abs) else {
            continue;
        };
        let Ok(tokens) = saule_lexer::Lexer::new(&source).tokenize() else {
            continue;
        };
        let Ok(imported) = saule_parser::parse(tokens) else {
            continue;
        };

        let (reg, ifaces, enums) = saule_semantic::build_registry(&imported);
        let funcs = saule_semantic::build_function_registry(&imported);
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

        // Barrel module: an `init.sau` re-exports what *it* imports, so the
        // metadata for the names we just pulled in lives one level deeper.
        // Recurse into its own imports (relative to the barrel's folder) and
        // fold the result in.
        if is_init_module(&abs) && depth < MAX_BARREL_DEPTH && visited.insert(abs.clone()) {
            let sub_dir = abs
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let sub = collect_import_seed_inner(&imported, &sub_dir, visited, depth + 1);
            merge_barrel_seed(&mut seed, sub, names);
        }
    }

    seed
}

/// Fold a barrel module's seed into `seed`, honouring the importer's name
/// filter: a glob takes everything, a name list takes only what it asked
/// for (renamed by any `as` clause).
pub(crate) fn merge_barrel_seed(
    seed: &mut saule_semantic::ModuleSeed,
    sub: saule_semantic::ModuleSeed,
    names: &ImportNames,
) {
    match names {
        ImportNames::All => {
            for (k, v) in sub.classes {
                seed.classes.entry(k).or_insert(v);
            }
            for (k, v) in sub.interfaces {
                seed.interfaces.entry(k).or_insert(v);
            }
            for (k, v) in sub.enums {
                seed.enums.entry(k).or_insert(v);
            }
            for (k, v) in sub.functions {
                seed.functions.entry(k).or_insert(v);
            }
            for (k, v) in sub.variables {
                seed.variables.entry(k).or_insert(v);
            }
        }
        ImportNames::List(items) => {
            for (orig, alias) in items {
                let local = alias.clone().unwrap_or_else(|| orig.clone());
                if let Some(v) = sub.classes.get(orig).cloned() {
                    seed.classes.entry(local.clone()).or_insert(v);
                }
                if let Some(v) = sub.interfaces.get(orig).cloned() {
                    seed.interfaces.entry(local.clone()).or_insert(v);
                }
                if let Some(v) = sub.enums.get(orig).cloned() {
                    seed.enums.entry(local.clone()).or_insert(v);
                }
                if let Some(v) = sub.functions.get(orig).cloned() {
                    seed.functions.entry(local.clone()).or_insert(v);
                }
                if let Some(v) = sub.variables.get(orig).cloned() {
                    seed.variables.entry(local).or_insert(v);
                }
            }
        }
    }
}

/// Resolve which `(original_name, local_alias)` pairs come into the
/// importing module's scope from one `import` statement.
pub(crate) fn collect_import_aliases(
    imported: &Module,
    names: &ImportNames,
) -> Vec<(String, String)> {
    match names {
        ImportNames::All => imported
            .stmts
            .iter()
            .filter_map(|s| match &s.value {
                Stmt::Decl(d) => exported_name(&d.value).map(|n| (n.to_string(), n.to_string())),
                _ => None,
            })
            .collect(),
        ImportNames::List(items) => items
            .iter()
            .map(|(orig, alias)| (orig.clone(), alias.clone().unwrap_or_else(|| orig.clone())))
            .collect(),
    }
}

/// Native-package counterpart of [`collect_import_aliases`]: wildcards
/// pull in every name the package declares via
/// [`crate::native_packages::NativePackage::exports`].
pub(crate) fn collect_native_aliases(
    pkg: &crate::native_packages::NativePackage,
    names: &ImportNames,
) -> Vec<(String, String)> {
    match names {
        ImportNames::All => pkg
            .exports
            .iter()
            .map(|n| ((*n).to_string(), (*n).to_string()))
            .collect(),
        ImportNames::List(items) => items
            .iter()
            .map(|(orig, alias)| (orig.clone(), alias.clone().unwrap_or_else(|| orig.clone())))
            .collect(),
    }
}
