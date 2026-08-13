//! The resolved project, and the process-wide "which project are we in?"
//! slot the interpreter consults.
//!
//! [`ProjectInfo`] is populated once at startup from `saule.config` and read
//! by:
//!   * `module::resolve_import_path` — adds `src_dirs` and `dependencies` as
//!     fallback roots for `import ... from "..."`.
//!   * `module::load_module_inner` — uses [`pretty_path`] to label a module
//!     `<project>/<rel>` instead of by absolute path.
//!   * `stdlib::project` — exposes the same data to user code as `Project`.
//!
//! All fields are best-effort: in single-file mode there is no config, [`get`]
//! returns `None`, and every consumer falls back to its previous behaviour.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct ProjectInfo {
    pub name: String,
    pub version: String,
    pub root: PathBuf,
    /// Extra source roots searched after the importing file's sibling
    /// directory. Always absolute.
    ///
    /// Empty when the config omits `src_dirs:`, and that emptiness is
    /// meaningful — it says "no *extra* roots", not "look in `src/`". Use
    /// [`crate::src_dirs_or_default`] where the other reading is wanted.
    pub src_dirs: Vec<PathBuf>,
    /// External library projects this project depends on, each resolved by
    /// reading the target project's own config.
    pub dependencies: Vec<Dependency>,
}

/// One resolved dependency: a named, external Saule project whose `src_dirs`
/// are made available to `import "<name>/..."` lookups.
#[derive(Debug, Clone)]
pub struct Dependency {
    /// Prefix used in imports. From the dep's own `name:`, falling back to
    /// its directory name.
    pub name: String,
    /// Absolute path to the dep's project root.
    pub root: PathBuf,
    /// Absolute `src_dirs` of the dep.
    pub src_dirs: Vec<PathBuf>,
}

thread_local! {
    static PROJECT: RefCell<Option<ProjectInfo>> = const { RefCell::new(None) };
}

/// Install the current project for this thread.
///
/// Thread-local rather than global because the interpreter is `Rc`-based and
/// thread-confined anyway; a language server handling requests on a tokio
/// worker pool must therefore call this on whichever thread is about to run
/// analysis, not once at startup.
pub fn set(info: ProjectInfo) {
    PROJECT.with(|p| *p.borrow_mut() = Some(info));
}

/// The current project, if one has been installed on this thread.
pub fn get() -> Option<ProjectInfo> {
    PROJECT.with(|p| p.borrow().clone())
}

/// Forget the current project. Used by tests that must not leak project
/// state into whatever runs next on the same thread.
pub fn clear() {
    PROJECT.with(|p| *p.borrow_mut() = None);
}

/// Render `abs_path` as `<project_name>/<relative>` when it lives under the
/// project root and the project has a non-empty name; otherwise return the
/// absolute path unchanged.
pub fn pretty_path(abs_path: &Path) -> String {
    if let Some(info) = get()
        && !info.name.is_empty()
        && let Ok(rel) = abs_path.strip_prefix(&info.root)
    {
        return format!("{}/{}", info.name, rel.display());
    }
    abs_path.display().to_string()
}
