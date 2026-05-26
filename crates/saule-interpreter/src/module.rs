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

use saule_ast::{Decl, Module, Spanned, Stmt};

use crate::env::Environment;
use crate::error::RuntimeError;
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
    let base = dir.join(&normalised);

    let candidates = [
        base.with_extension("sau"),
        base.with_extension("saule"),
        base.join("init.sau"),
        base.join("init.saule"),
        base.clone(),
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
            message: format!(
                "circular import detected at `{}`",
                abs_path.display()
            ),
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

    let tokens = saule_lexer::Lexer::new(&source)
        .tokenize()
        .map_err(|e| RuntimeError::ImportError {
            message: format!("lex error in `{}`: {e}", abs_path.display()),
            span: import_span.clone(),
        })?;

    let module = saule_parser::parse(tokens).map_err(|e| RuntimeError::ImportError {
        message: format!("parse error in `{}`: {e}", abs_path.display()),
        span: import_span.clone(),
    })?;

    let errors = crate::typeck::check(&module);
    if let Some(first) = errors.into_iter().next() {
        return Err(RuntimeError::ImportError {
            message: format!("type error in `{}`: {first}", abs_path.display()),
            span: import_span,
        });
    }

    let dir = abs_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let env = Environment::with_prelude_and_context(Some(dir), Some(loader.clone()));

    crate::run_in(&module, &env)?;

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
