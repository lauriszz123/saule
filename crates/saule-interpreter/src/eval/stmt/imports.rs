//! `import ... from "path"` execution.

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::ImportNames;

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::module;

use super::super::Flow;

/// Execute `import ... from "path"`:
///   1. Resolve `path` relative to the importing file's directory.
///   2. Load (or fetch cached) exports for that module via the shared
///      [`module::ModuleLoader`].
///   3. Bind the requested names — optionally aliased — into `env`.
pub(super) fn exec_import(
    names: &ImportNames,
    path: &str,
    env: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    let loader = env
        .borrow()
        .loader()
        .ok_or_else(|| RuntimeError::ImportError {
            message:
                "no module loader available — running this file with `saule run` should attach one"
                    .to_string(),
            span: span.clone(),
        })?;
    let dir = env
        .borrow()
        .module_dir()
        .ok_or_else(|| RuntimeError::ImportError {
            message: "cannot resolve relative import: current file has no known directory"
                .to_string(),
            span: span.clone(),
        })?;

    let abs = module::resolve_import_path(&dir, path).ok_or_else(|| RuntimeError::ImportError {
        message: format!(
            "could not find module `{path}` (looked for `.sau` / `.saule` / `init.sau`)"
        ),
        span: span.clone(),
    })?;

    let exports = module::load_module(&abs, &loader, span.clone())?;

    match names {
        ImportNames::All => {
            for (n, v) in &exports.values {
                env.borrow_mut().define(n.clone(), v.clone());
            }
        }
        ImportNames::List(list) => {
            for (n, alias) in list {
                let v =
                    exports
                        .values
                        .get(n)
                        .cloned()
                        .ok_or_else(|| RuntimeError::ImportError {
                            message: format!("`{n}` is not exported from `{}`", abs.display()),
                            span: span.clone(),
                        })?;
                let bind = alias.clone().unwrap_or_else(|| n.clone());
                env.borrow_mut().define(bind, v);
            }
        }
    }

    Ok(Flow::nil())
}
