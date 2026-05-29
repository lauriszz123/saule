//! Module loader for `import X from "path"`.
//!
//! Each `.sau` (or `.saule`) file becomes a *module*. Loading a module
//! lexes, parses, type-checks, and runs its top-level statements in a fresh
//! environment seeded with the prelude. The set of names declared with
//! `export` is captured and handed back to whoever requested the import.
//!
//! Results are memoised: importing the same file twice reuses the same
//! [`ModuleExports`] (and so the same `Class` / `Function` `Rc`s) instead of
//! re-executing the file. A `loading` set guards against circular imports.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use saule_ast::{Decl, ImportNames, Module, Spanned, Stmt};

use crate::env::Environment;
use crate::error::{ImportedDiagnostic, RuntimeError};
use crate::value::Value;

/// The publicly importable surface of a loaded module.
#[derive(Debug, Default, Clone)]
pub struct ModuleExports {
    pub values: HashMap<String, Value>,
}

/// Shared cache of already-loaded modules plus an in-flight set used to
/// detect circular imports.
#[derive(Debug, Default)]
pub struct ModuleLoader {
    cache: HashMap<PathBuf, ModuleExports>,
    loading: HashSet<PathBuf>,
}

impl ModuleLoader {
    pub fn new() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::default()))
    }
}

thread_local! {
    /// Source attached to functions/methods built while this slot is set.
    /// `load_module_inner` populates it for the duration of the imported
    /// module's top-level execution so every `FunctionObject` born there
    /// can later wrap its own runtime errors with the right source snippet.
    static ACTIVE_MODULE_SOURCE: RefCell<Option<Rc<miette::NamedSource<String>>>> =
        const { RefCell::new(None) };
}

/// Returns the module source currently being loaded, if any. Consulted by
/// `FunctionObject` constructors in `eval/stmt.rs` and `eval/expr.rs`.
pub fn active_module_source() -> Option<Rc<miette::NamedSource<String>>> {
    ACTIVE_MODULE_SOURCE.with(|s| s.borrow().clone())
}

fn set_active_module_source(
    new: Option<Rc<miette::NamedSource<String>>>,
) -> Option<Rc<miette::NamedSource<String>>> {
    ACTIVE_MODULE_SOURCE.with(|s| std::mem::replace(&mut *s.borrow_mut(), new))
}

/// Resolve a `"path"` literal as it appears in an `import ... from "path"`
/// against the importing file's directory. Tries, in order:
///   1. `<dir>/<path>.sau`
///   2. `<dir>/<path>.saule`
///   3. `<dir>/<path>/init.sau`
///   4. `<dir>/<path>/init.saule`
///   5. `<dir>/<path>` (already has an extension)
///
/// Both `/` and `.` are accepted as path separators inside the literal so
/// `"entities/Player"` and `"entities.Player"` both work.
pub fn resolve_import_path(dir: &Path, raw: &str) -> Option<PathBuf> {
    let normalised = raw.replace('.', "/");

    if let Some(hit) = try_resolve_base(&dir.join(&normalised)) {
        return Some(hit);
    }

    // Project-wide `src_dirs:` fallback. These are absolute, so we look the
    // import up under each in turn before giving up.
    if let Some(info) = crate::project::get() {
        for src_dir in &info.src_dirs {
            if let Some(hit) = try_resolve_base(&src_dir.join(&normalised)) {
                return Some(hit);
            }
        }

        // Dependency lookup: if the first path segment names a dep, strip
        // it and resolve the remainder under that dep's `src_dirs`.
        if let Some((head, rest)) = normalised.split_once('/') {
            for dep in &info.dependencies {
                if dep.name == head {
                    for src_dir in &dep.src_dirs {
                        if let Some(hit) = try_resolve_base(&src_dir.join(rest)) {
                            return Some(hit);
                        }
                    }
                }
            }
        } else {
            // Bare `import X from "json"` — match the dep itself; look for
            // an `init.sau` (or single-file project) under its src_dirs.
            for dep in &info.dependencies {
                if dep.name == normalised {
                    for src_dir in &dep.src_dirs {
                        if let Some(hit) = try_resolve_base(src_dir) {
                            return Some(hit);
                        }
                    }
                }
            }
        }
    }

    None
}

fn try_resolve_base(base: &Path) -> Option<PathBuf> {
    let candidates = [
        base.with_extension("sau"),
        base.with_extension("saule"),
        base.join("init.sau"),
        base.join("init.saule"),
        base.to_path_buf(),
    ];

    for candidate in candidates {
        if candidate.is_file() {
            return std::fs::canonicalize(&candidate).ok().or(Some(candidate));
        }
    }
    None
}

/// Load (or return cached) exports for the module at `abs_path`.
///
/// `loader` must be the same `Rc` that lives on the importing environment
/// — newly created child environments for the imported module borrow it so
/// transitive `import` statements share the same cache.
pub fn load_module(
    abs_path: &Path,
    loader: &Rc<RefCell<ModuleLoader>>,
    import_span: std::ops::Range<usize>,
) -> Result<ModuleExports, RuntimeError> {
    if let Some(hit) = loader.borrow().cache.get(abs_path) {
        return Ok(hit.clone());
    }

    if loader.borrow().loading.contains(abs_path) {
        return Err(RuntimeError::ImportError {
            message: format!("circular import detected at `{}`", abs_path.display()),
            span: import_span,
        });
    }
    loader.borrow_mut().loading.insert(abs_path.to_path_buf());

    let result = load_module_inner(abs_path, loader, import_span.clone());

    loader.borrow_mut().loading.remove(abs_path);

    let exports = result?;
    loader
        .borrow_mut()
        .cache
        .insert(abs_path.to_path_buf(), exports.clone());
    Ok(exports)
}

fn load_module_inner(
    abs_path: &Path,
    loader: &Rc<RefCell<ModuleLoader>>,
    import_span: std::ops::Range<usize>,
) -> Result<ModuleExports, RuntimeError> {
    let source = std::fs::read_to_string(abs_path).map_err(|e| RuntimeError::ImportError {
        message: format!("could not read `{}`: {e}", abs_path.display()),
        span: import_span.clone(),
    })?;

    let file_label = crate::project::pretty_path(abs_path);
    let wrap = |inner: &dyn miette::Diagnostic| RuntimeError::ImportFailed {
        module_label: file_label.clone(),
        import_span: import_span.clone(),
        inner: Box::new(ImportedDiagnostic::from_inner(
            inner,
            file_label.clone(),
            source.clone(),
        )),
    };

    let tokens = saule_lexer::Lexer::new(&source)
        .tokenize()
        .map_err(|e| wrap(&e))?;

    let module = saule_parser::parse(tokens).map_err(|e| wrap(&e))?;

    let dir = abs_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // Pre-collect class/interface/enum metadata from every directly-
    // imported file so the typechecker sees imported methods' signatures.
    // The seed is best-effort: if an import can't be resolved or parsed,
    // we skip it silently — semantic/typeck will surface the user-facing
    // issue separately, and missing seed entries just mean we can't
    // statically check those particular calls.
    let seed = collect_import_seed(&module, &dir);

    // Pipeline: semantic (registry build + field-init + control-flow) runs
    // first; if it produces *any* error we don't even attempt typecheck —
    // the type pass assumes a structurally valid module.
    let sem_errors = saule_semantic::analyze_with_seed(&module, seed);
    if let Some(first) = sem_errors.into_iter().next() {
        return Err(wrap(&first));
    }

    let errors = saule_typeck::check(&module);
    if let Some(first) = errors.into_iter().next() {
        return Err(wrap(&first));
    }

    let env = Environment::with_prelude_and_context(Some(dir), Some(loader.clone()));

    // Park the module's `NamedSource` for the duration of its top-level
    // execution so every `FunctionObject` constructed by `class`/`fn`
    // declarations carries it. Restore the previous slot on the way out so
    // nested imports don't trample each other.
    let module_src = Rc::new(miette::NamedSource::new(file_label.clone(), source.clone()));
    let prev_src = set_active_module_source(Some(module_src));

    // Runtime errors from the imported module's top-level: wrap them too,
    // *unless* they're already an ImportFailed (transitive import — keep
    // the original to preserve the deepest source attachment).
    let run_result = crate::run_in(&module, &env);

    set_active_module_source(prev_src);

    run_result.map_err(|e| match e {
        RuntimeError::ImportFailed { .. } | RuntimeError::InModule { .. } => e,
        other => wrap(&other),
    })?;

    Ok(collect_exports(&module, &env))
}

/// Walk the module's top-level declarations and copy out everything that
/// carries `export`. Anything not exported stays private to the module.
fn collect_exports(module: &Module, env: &Rc<RefCell<Environment>>) -> ModuleExports {
    let mut exports = ModuleExports::default();
    for stmt in &module.stmts {
        if let Stmt::Decl(decl) = &stmt.value {
            if let Some(name) = exported_name(&decl.value) {
                if let Some(value) = env.borrow().get(name) {
                    exports.values.insert(name.to_string(), value);
                }
            }
        }
    }
    exports
}

fn exported_name(decl: &Decl) -> Option<&str> {
    match decl {
        Decl::Function {
            exported: true,
            name,
            ..
        }
        | Decl::Class {
            exported: true,
            name,
            ..
        }
        | Decl::Interface {
            exported: true,
            name,
            ..
        }
        | Decl::Enum {
            exported: true,
            name,
            ..
        } => Some(name),
        _ => None,
    }
}

// Silence unused-import lint when `Spanned` re-export is not needed
// (kept here in case future loader paths need explicit span info).
#[allow(dead_code)]
fn _spanned_marker(_: &Spanned<Stmt>) {}

// ──────────────────────────────────────────────────────────────────────────────
// Cross-module typecheck seed
// ──────────────────────────────────────────────────────────────────────────────

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
    let mut seed = saule_semantic::ModuleSeed::default();

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let Decl::Import { names, path } = &d.value else {
            continue;
        };

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
                seed.enums.entry(alias).or_insert(info);
            }
        }
    }

    seed
}

/// Resolve which `(original_name, local_alias)` pairs come into the
/// importing module's scope from one `import` statement.
fn collect_import_aliases(imported: &Module, names: &ImportNames) -> Vec<(String, String)> {
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
