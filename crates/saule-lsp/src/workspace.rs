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
        let Ok(dep_root) = dep_root.canonicalize() else { continue };

        let Ok(cfg) = fs::read_to_string(dep_root.join("saule.config")) else { continue };
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
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return PathBuf::from(home).join(rest);
        }
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
