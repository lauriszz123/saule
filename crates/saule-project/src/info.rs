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

/// `abs_path` spelled the way user code can use it.
///
/// Paths reaching Saule code are canonical, and on Windows canonicalising
/// yields a verbatim path — `\\?\C:\proj`. That prefix is what lets a path
/// exceed 260 characters, but it also switches off the normalisation that
/// would otherwise accept `/` as a separator. Saule has no path type, so the
/// only way user code can build a subpath is string concatenation, and
/// `Project.root .. "/" .. "assets/icon.ttf"` under a verbatim root names a
/// file that does not exist — silently, since it looks absolute and correct.
///
/// Stripping the prefix is only safe where the shorter spelling reaches the
/// same file, so it is limited to drive-letter and UNC-share paths that fit
/// inside `MAX_PATH`. A long path or a device path (`\\?\Volume{..}`) keeps
/// the prefix: there the prefix is load-bearing, and a broken join is better
/// than a path that cannot be opened at all.
///
/// The internal spelling is deliberately left alone — `canonical` feeds
/// `src_dirs` comparisons and the database's read set, which only agree as
/// long as every side canonicalises identically.
pub fn user_path(abs_path: &Path) -> String {
    strip_verbatim(&abs_path.display().to_string())
}

/// The string half of [`user_path`], split out so it is testable off Windows.
fn strip_verbatim(path: &str) -> String {
    /// Windows' classic path limit. The prefix is only redundant below it.
    const MAX_PATH: usize = 260;

    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        let share = format!(r"\\{rest}");
        if share.len() < MAX_PATH {
            return share;
        }
        return path.to_string();
    }

    if let Some(rest) = path.strip_prefix(r"\\?\") {
        // `C:\..` only. Anything else behind the prefix — a volume GUID, a
        // device name — has no prefix-free spelling to fall back to.
        let bytes = rest.as_bytes();
        let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';

        if drive && rest.len() < MAX_PATH {
            return rest.to_string();
        }
    }

    path.to_string()
}

/// Render `abs_path` as `<project_name>/<relative>` when it lives under the
/// project root and the project has a non-empty name; otherwise return the
/// absolute path unchanged.
///
/// The relative half is joined with `/` on **every** platform. This is a
/// display-only module label — the one consumer puts it in a diagnostic
/// header (`module.rs`) and nothing ever opens it or parses it back — and
/// the project half was already `/`-joined, so deferring to the platform
/// separator for the rest produced `demo/src\main.sau` on Windows: neither
/// convention, and different error text on different machines.
///
/// The path *outside* the project is left alone, because that branch really
/// is a filesystem path the reader may want to paste into a shell.
pub fn pretty_path(abs_path: &Path) -> String {
    if let Some(info) = get()
        && !info.name.is_empty()
        && let Ok(rel) = abs_path.strip_prefix(&info.root)
    {
        let rel: Vec<std::borrow::Cow<'_, str>> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect();
        return format!("{}/{}", info.name, rel.join("/"));
    }
    abs_path.display().to_string()
}
