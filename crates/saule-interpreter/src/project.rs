//! Project info published by the CLI when running in project mode.
//!
//! Populated once at startup from `saule.config` and consulted by:
//!   * [`module::resolve_import_path`] — adds the project's `src_dirs` as
//!     fallback roots for `import ... from "..."` lookups.
//!   * [`module::load_module_inner`] — uses [`pretty_path`] to render the
//!     module label as `<project>/<rel>` instead of an absolute path.
//!   * `stdlib::project` — exposes the same data to user code via the
//!     built-in `Project` static class.
//!
//! All fields are best-effort: in single-file mode (no config) nothing is
//! set and every consumer falls back to its previous behaviour.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct ProjectInfo {
    pub name: String,
    pub version: String,
    pub root: PathBuf,
    /// Extra source roots searched after the importing file's sibling
    /// directory. Always absolute.
    pub src_dirs: Vec<PathBuf>,
}

thread_local! {
    static PROJECT: RefCell<Option<ProjectInfo>> = const { RefCell::new(None) };
}

pub fn set(info: ProjectInfo) {
    PROJECT.with(|p| *p.borrow_mut() = Some(info));
}

pub fn get() -> Option<ProjectInfo> {
    PROJECT.with(|p| p.borrow().clone())
}

/// Render `abs_path` as `<project_name>/<relative>` when it lives under the
/// project root and the project has a non-empty name; otherwise return the
/// absolute path unchanged.
pub fn pretty_path(abs_path: &Path) -> String {
    if let Some(info) = get() {
        if !info.name.is_empty() {
            if let Ok(rel) = abs_path.strip_prefix(&info.root) {
                return format!("{}/{}", info.name, rel.display());
            }
        }
    }
    abs_path.display().to_string()
}

