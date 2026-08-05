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

mod resolve;
mod seed;
#[cfg(test)]
mod tests;

pub use resolve::*;
pub use seed::*;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use saule_ast::{Decl, Module, Spanned, Stmt};

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

    // Native package shortcut — the sentinel path encodes the package
    // name; build the exports by running the package's `install`
    // against a scratch env and harvesting its declared exports.
    if let Some(name) = crate::native_packages::name_from_sentinel(abs_path) {
        let pkg =
            crate::native_packages::lookup(name).ok_or_else(|| RuntimeError::ImportError {
                message: format!("native package `{name}` is no longer registered"),
                span: import_span,
            })?;
        let exports = crate::native_packages::build_exports(pkg);
        loader
            .borrow_mut()
            .cache
            .insert(abs_path.to_path_buf(), exports.clone());
        return Ok(exports);
    }

    // Dynamic (manifest-described) native package shortcut — loads the
    // shared library on first import and wraps its exported symbols.
    if let Some(name) = crate::dynamic_packages::name_from_sentinel(abs_path) {
        let exports = crate::dynamic_packages::build_exports(name, import_span)?;
        loader
            .borrow_mut()
            .cache
            .insert(abs_path.to_path_buf(), exports.clone());
        return Ok(exports);
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

    let env = Environment::with_prelude_and_context(Some(dir.clone()), Some(loader.clone()));

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

    Ok(collect_exports(
        &module,
        &env,
        &dir,
        loader,
        is_init_module(abs_path),
    ))
}

/// Walk the module's top-level declarations and copy out everything that
/// carries `export`. Anything not exported stays private to the module.
///
/// When `reexport_imports` is set (the module is an `init.sau` barrel), the
/// names brought in by its `import` statements are published too — that is
/// what lets
///
/// ```text
/// -- some/folder/module/init.sau
/// import * from "view"
/// ```
///
/// make `View` visible to `import * from "some/folder/module"`.
fn collect_exports(
    module: &Module,
    env: &Rc<RefCell<Environment>>,
    dir: &Path,
    loader: &Rc<RefCell<ModuleLoader>>,
    reexport_imports: bool,
) -> ModuleExports {
    let mut exports = ModuleExports::default();
    for stmt in &module.stmts {
        let Stmt::Decl(decl) = &stmt.value else {
            continue;
        };

        if let Some(name) = exported_name(&decl.value) {
            if let Some(value) = env.borrow().get(name) {
                exports.values.insert(name.to_string(), value);
            }
            continue;
        }

        if reexport_imports && let Decl::Import { names, path, .. } = &decl.value {
            for local in imported_local_names(names, path, dir, loader) {
                if let Some(value) = env.borrow().get(&local) {
                    exports.values.insert(local, value);
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
        }
        | Decl::Variable {
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
