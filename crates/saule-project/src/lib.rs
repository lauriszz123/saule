//! `saule.config`: parsing it, finding it, and resolving what it points at.
//!
//! This crate exists because there used to be two of it. The CLI needed a
//! config parser to run a project; the language server needed one to analyse
//! a workspace, could not reach the CLI's (private, and depending on a binary
//! crate inverts the dependency direction), and grew its own. The two drifted
//! in exactly the ways two hand-maintained parsers of the same format drift:
//! the server's read four of the seven keys, and the CLI's file walker knew
//! about only one of the two source extensions.
//!
//! Everything downstream of `saule.config` therefore lives here — the format,
//! project discovery, dependency resolution, source-file scanning, and the
//! [`ProjectInfo`] the interpreter consults during import resolution — and
//! `saule-cli`, `saule-lsp` and `saule-interpreter` all read it from one
//! place.
//!
//! ```no_run
//! # use std::path::Path;
//! let root = saule_project::find_root(Path::new("src/main.sau")).unwrap();
//! let info = saule_project::load(&root).unwrap();
//! saule_project::set(info);
//! ```

mod config;
mod deps;
mod info;
mod scan;

use std::path::{Path, PathBuf};

pub use config::{Config, Kind};
pub use deps::{
    expand_tilde, resolve_dependencies, resolve_dependencies_lenient, resolve_dependency,
};
pub use info::{Dependency, ProjectInfo, clear, get, pretty_path, set};
pub use scan::{SOURCE_EXTENSIONS, find_root, is_source_file, scan_all, scan_sources};

/// The one filename this crate is about.
pub const CONFIG_FILE: &str = "saule.config";

/// The default entry point for a project whose config omits `entry:`.
pub const DEFAULT_ENTRY: &str = "src/main.sau";

/// The declared `src_dirs:` as absolute paths, or `<root>/src` when the
/// config omits the key.
///
/// The defaulting is *not* applied to [`ProjectInfo::src_dirs`], and the
/// difference is load-bearing. To import resolution an empty list means "no
/// extra roots beyond the importing file's own directory", which is a
/// coherent reading that `run` has always relied on. It is not a coherent
/// reading of "where do this project's files live" — so a file walker, and a
/// dependency's exposed sources, use this instead.
pub fn src_dirs_or_default(root: &Path, config: &Config) -> Vec<PathBuf> {
    if config.src_dirs.is_empty() {
        vec![root.join("src")]
    } else {
        config.src_dirs.iter().map(|s| root.join(s)).collect()
    }
}

impl ProjectInfo {
    /// Build the interpreter's view of a project from its root and config,
    /// failing if any dependency cannot be resolved.
    pub fn resolve(root: &Path, config: &Config) -> Result<ProjectInfo, String> {
        let root = canonical(root);
        Ok(ProjectInfo {
            name: config.name.clone().unwrap_or_default(),
            version: config.version.clone().unwrap_or_default(),
            src_dirs: config.src_dirs.iter().map(|s| root.join(s)).collect(),
            dependencies: resolve_dependencies(&root, &config.dependencies)?,
            root,
        })
    }

    /// [`ProjectInfo::resolve`], skipping dependencies that fail to resolve.
    pub fn resolve_lenient(root: &Path, config: &Config) -> ProjectInfo {
        let root = canonical(root);
        ProjectInfo {
            name: config.name.clone().unwrap_or_default(),
            version: config.version.clone().unwrap_or_default(),
            src_dirs: config.src_dirs.iter().map(|s| root.join(s)).collect(),
            dependencies: resolve_dependencies_lenient(&root, &config.dependencies),
            root,
        }
    }

    /// Every source file belonging to this project, following
    /// [`src_dirs_or_default`] rather than [`ProjectInfo::src_dirs`].
    pub fn source_files(&self, config: &Config) -> Vec<PathBuf> {
        scan_all(&src_dirs_or_default(&self.root, config))
    }
}

/// Read and resolve the project rooted at `root`, tolerating broken
/// dependencies. `None` only if there is no readable config there.
pub fn load(root: &Path) -> Option<ProjectInfo> {
    let config = Config::read_in(root).ok()?;
    Some(ProjectInfo::resolve_lenient(root, &config))
}

/// Canonicalise so that every `src_dirs` and `pretty_path` comparison
/// downstream compares like with like; fall back to the path as given when
/// it does not exist yet.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// A scratch directory tree. `tempfile` is not a dependency of this
    /// crate and every test here needs real directory entries to walk.
    pub(crate) struct Scratch(PathBuf);

    impl Scratch {
        pub(crate) fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "saule-project-{tag}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create scratch dir");
            // Canonicalised up front: on macOS `/tmp` is a symlink, and a
            // test comparing a resolved path against `root().join(..)`
            // would otherwise compare `/private/tmp/...` against `/tmp/...`.
            Scratch(fs::canonicalize(&path).expect("canonicalize scratch dir"))
        }

        pub(crate) fn root(&self) -> PathBuf {
            self.0.clone()
        }

        /// Create `rel` and its parent directories.
        pub(crate) fn write(&self, rel: &str, contents: &str) {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, contents).expect("write file");
        }

        pub(crate) fn canonical(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    use super::*;

    #[test]
    fn an_absent_src_dirs_means_no_extra_roots_but_walks_src() {
        let s = Scratch::new("srcdirs-default");
        s.write("saule.config", "name: \"demo\"");
        s.write("src/main.sau", "");

        let config = Config::read_in(&s.root()).expect("read");
        let info = ProjectInfo::resolve(&s.root(), &config).expect("resolve");

        assert!(
            info.src_dirs.is_empty(),
            "import resolution gets no extra roots"
        );
        assert_eq!(info.source_files(&config), [s.canonical("src/main.sau")]);
    }

    #[test]
    fn load_resolves_dependencies_and_names_the_project() {
        let s = Scratch::new("load");
        s.write(
            "app/saule.config",
            "name: \"app\"\nversion: \"2.0\"\ndependencies: [\"../lib\"]",
        );
        s.write("lib/saule.config", "name: \"lib\"");

        let info = load(&s.canonical("app")).expect("load");
        assert_eq!(info.name, "app");
        assert_eq!(info.version, "2.0");
        assert_eq!(info.dependencies.len(), 1);
        assert_eq!(info.dependencies[0].name, "lib");
    }

    #[test]
    fn load_is_none_without_a_config() {
        let s = Scratch::new("load-none");
        s.write("src/main.sau", "");
        assert!(load(&s.root()).is_none());
    }

    /// `pretty_path` is how a module names itself in an error message, and
    /// it is the reason the root is canonicalised on the way in.
    #[test]
    fn pretty_path_labels_files_inside_the_project() {
        let s = Scratch::new("pretty");
        s.write("saule.config", "name: \"demo\"");
        s.write("src/main.sau", "");

        set(load(&s.root()).expect("load"));
        assert_eq!(
            pretty_path(&s.canonical("src/main.sau")),
            format!("demo/{}", Path::new("src/main.sau").display())
        );

        let outside = s.root().parent().expect("parent").join("elsewhere.sau");
        assert_eq!(pretty_path(&outside), outside.display().to_string());
        clear();
    }
}
