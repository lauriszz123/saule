//! Workspace discovery for the LSP: locate `saule.config`, parse it into
//! the interpreter's `ProjectInfo`, and recursively collect every `.sau`
//! file under a workspace root.
//!
//! Mirrors `crates/saule-cli/src/project.rs` in miniature — duplicated
//! rather than moved because the CLI's parser is private and pulling in
//! the entire CLI crate just for config IO would invert the dependency
//! direction (LSP currently depends only on the analysis crates).

use std::fs;
use std::path::{Path, PathBuf};

use saule_interpreter::project::{Dependency, ProjectInfo};

/// Walk up from `start` looking for the nearest directory that contains
/// a `saule.config`. Returns that directory, or `None` if none of the
/// ancestors qualify.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        if dir.join("saule.config").is_file() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// Read and parse the `saule.config` at `root`, resolving `src_dirs:` and
/// `dependencies:` into absolute paths. Returns `None` if the file is
/// missing or unreadable.
pub fn load_project(root: &Path) -> Option<ProjectInfo> {
    let cfg = fs::read_to_string(root.join("saule.config")).ok()?;
    let (name, version, src_dirs_raw, deps_raw) = parse_kv(&cfg);

    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let src_dirs: Vec<PathBuf> = src_dirs_raw.iter().map(|s| root.join(s)).collect();
    let dependencies = resolve_deps(&root, &deps_raw);

    Some(ProjectInfo {
        name: name.unwrap_or_default(),
        version: version.unwrap_or_default(),
        root,
        src_dirs,
        dependencies,
    })
}

/// Recursively collect every `.sau` / `.saule` file under `root`,
/// skipping hidden directories, `target/`, and `node_modules/`.
pub fn scan_saule_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            walk(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e == "sau" || e == "saule")
            .unwrap_or(false)
        {
            if let Ok(canon) = path.canonicalize() {
                out.push(canon);
            } else {
                out.push(path);
            }
        }
    }
}

// ── config parsing (mirror of saule-cli/src/project.rs) ──────────────

fn parse_kv(text: &str) -> (Option<String>, Option<String>, Vec<String>, Vec<String>) {
    let mut name = None;
    let mut version = None;
    let mut src_dirs = Vec::new();
    let mut dependencies = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("--") {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim();
        match k {
            "name" => name = Some(unquote(v)),
            "version" => version = Some(unquote(v)),
            "src_dirs" => src_dirs = parse_list(v),
            "dependencies" => dependencies = parse_list(v),
            _ => {}
        }
    }
    (name, version, src_dirs, dependencies)
}

fn resolve_deps(project_root: &Path, deps: &[String]) -> Vec<Dependency> {
    let mut out = Vec::new();
    for raw in deps {
        let expanded = expand_tilde(raw);
        let dep_root = if expanded.is_absolute() {
            expanded
        } else {
            project_root.join(expanded)
        };
        let Ok(dep_root) = dep_root.canonicalize() else {
            continue;
        };

        let Ok(cfg) = fs::read_to_string(dep_root.join("saule.config")) else {
            continue;
        };
        let (dep_name_opt, _, dep_src_raw, _) = parse_kv(&cfg);

        let name = dep_name_opt.filter(|s| !s.is_empty()).unwrap_or_else(|| {
            dep_root
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        let src_dirs: Vec<PathBuf> = if dep_src_raw.is_empty() {
            vec![dep_root.join("src")]
        } else {
            dep_src_raw.iter().map(|s| dep_root.join(s)).collect()
        };

        out.push(Dependency {
            name,
            root: dep_root,
            src_dirs,
        });
    }
    out
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

fn parse_list(raw: &str) -> Vec<String> {
    let inner = raw.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|p| unquote(p.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory tree. `tempfile` is not a dependency of this crate
    /// and a walker test needs real directory entries to walk.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let mut path = std::env::temp_dir();
            path.push(format!(
                "saule-lsp-scan-{tag}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create scratch dir");
            Scratch(path)
        }

        /// Create `rel` (and its parents) with enough content to be a real file.
        fn file(&self, rel: &str) {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, "class Main\nend\n").expect("write file");
        }

        fn scan_file_names(&self) -> Vec<String> {
            let mut names: Vec<String> = scan_saule_files(&self.0)
                .iter()
                .filter_map(|p| p.file_name().and_then(|s| s.to_str()))
                .map(str::to_string)
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The one that matters in practice: a Claude Code worktree under
    /// `.claude/` is a complete second copy of the repository. Walking into it
    /// would analyse — and publish diagnostics for — every file twice, against
    /// stale duplicates the user is not editing.
    #[test]
    fn skips_dot_directories() {
        let s = Scratch::new("dotdirs");
        s.file("src/real.sau");
        s.file(".claude/worktrees/adoring-cerf/tests/ui/duplicate.sau");
        s.file(".git/hooks/hook.sau");
        s.file(".idea/scratch.sau");

        assert_eq!(s.scan_file_names(), vec!["real.sau"]);
    }

    #[test]
    fn skips_build_output_directories() {
        let s = Scratch::new("buildout");
        s.file("src/real.sau");
        s.file("target/release/generated.sau");
        s.file("node_modules/some-pkg/bundled.sau");

        assert_eq!(s.scan_file_names(), vec!["real.sau"]);
    }

    #[test]
    fn finds_nested_files_and_both_extensions() {
        let s = Scratch::new("nested");
        s.file("top.sau");
        s.file("a/b/c/deep.saule");

        assert_eq!(s.scan_file_names(), vec!["deep.saule", "top.sau"]);
    }

    #[test]
    fn ignores_files_that_are_not_saule_sources() {
        let s = Scratch::new("exts");
        s.file("keep.sau");
        s.file("README.md");
        s.file("build.rs");
        s.file("saule.config");

        assert_eq!(s.scan_file_names(), vec!["keep.sau"]);
    }

    /// A dot-directory nested deeper than the root must be skipped too — the
    /// check is per directory component, not just the top level.
    #[test]
    fn skips_dot_directories_below_the_root() {
        let s = Scratch::new("deepdot");
        s.file("packages/app/src/real.sau");
        s.file("packages/app/.claude/worktrees/copy/duplicate.sau");

        assert_eq!(s.scan_file_names(), vec!["real.sau"]);
    }

    #[test]
    fn missing_root_yields_nothing_rather_than_panicking() {
        let s = Scratch::new("missing");
        let gone = s.0.join("does-not-exist");
        assert!(scan_saule_files(&gone).is_empty());
    }
}
