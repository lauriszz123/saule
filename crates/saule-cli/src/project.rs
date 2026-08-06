//! Project-mode bootstrap: read `saule.config`, configure the interpreter's
//! project context, then hand off to [`crate::run::run_file`] on the entry
//! point.

use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use crate::run::run_file;

/// A project after its `saule.config` has been read and
/// [`saule_interpreter::project::set`] has been called.
///
/// Produced by [`configure_project`] and consumed by both `run` (which wants
/// the entry point) and `check` (which wants every source file).
pub(crate) struct Project {
    /// Directories to search for this project's own sources, in declaration
    /// order.
    ///
    /// Not always the same list as [`saule_interpreter::project::ProjectInfo`]
    /// gets. A config that omits `src_dirs:` leaves that one empty, because
    /// import resolution treats it as "no *extra* roots beyond the importing
    /// file's own directory" — which is a coherent meaning, and one `run` has
    /// always relied on. It is not a coherent meaning for "where do this
    /// project's files live", so this list falls back to `<root>/src`, which
    /// is both the `entry:` default and what a *dependency*'s `src_dirs`
    /// already defaults to (see `resolve_dependencies`).
    pub src_dirs: Vec<PathBuf>,
    /// Absolute path to `entry:`. `None` for `kind: "library"`, which has no
    /// entry point by definition.
    pub entry: Option<PathBuf>,
}

impl Project {
    /// Every `.sau` file under `src_dirs`, sorted, deduplicated.
    ///
    /// `check` walks all of them rather than following imports from the entry
    /// point: a file no one imports yet is exactly the file whose errors you
    /// want to hear about before you import it.
    pub fn source_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for dir in &self.src_dirs {
            collect_sau_files(dir, &mut out);
        }
        out.sort();
        out.dedup();
        out
    }
}

fn collect_sau_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sau_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "sau") {
            out.push(path.canonicalize().unwrap_or(path));
        }
    }
}

pub(crate) fn run_project(dir: &Path) {
    let project = configure_project(dir, /* require_entry */ true);
    // `require_entry` guarantees this; a library exits inside `configure_project`.
    let entry = project.entry.unwrap_or_else(|| {
        eprintln!("error: project has no entry point");
        process::exit(1);
    });
    run_file(entry, true);
}

/// Read `saule.config`, validate it, install the interpreter's project
/// context, and report where the sources live.
///
/// `require_entry` distinguishes the two callers: `run` cannot proceed without
/// an entry point and exits with an explanation for a library, while `check`
/// is perfectly happy to check a library and just gets `entry: None`.
pub(crate) fn configure_project(dir: &Path, require_entry: bool) -> Project {
    let config_path = dir.join("saule.config");
    if !config_path.exists() {
        eprintln!(
            "error: no `saule.config` in `{}`\n\nRun `saule init <name>` to create one, or pass a file path.",
            dir.display()
        );
        process::exit(1);
    }

    let config = match read_config(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error reading {}: {e}", config_path.display());
            process::exit(1);
        }
    };

    // `kind:` decides whether this project is runnable at all. Checked before
    // anything else so a library reports what it is, rather than failing later
    // with a confusing "entry `src/main.sau` does not exist".
    //
    // A library is only an error for `run`. `check` sets `require_entry =
    // false` because type-checking a library is exactly what a library author
    // wants — arguably more than an app author does.
    let is_library = matches!(config.kind.as_deref(), Some("library"));
    match config.kind.as_deref() {
        None | Some("app") | Some("library") => {}
        Some(other) => {
            eprintln!(
                "error: unknown `kind: \"{other}\"` in saule.config — expected \"app\" or \"library\""
            );
            process::exit(1);
        }
    }
    if is_library && require_entry {
        let name = config.name.as_deref().unwrap_or("this project");
        eprintln!(
            "error: `{name}` is a library and has no entry point\n\n\
             Libraries are imported by other projects rather than run. Add it to a \n\
             project's `dependencies:` and `import` it, or set `kind: \"app\"` and an \n\
             `entry:` in saule.config to make it runnable."
        );
        process::exit(1);
    }

    // Canonicalise the project root so every `pretty_path` / `src_dirs`
    // comparison downstream is comparing apples to apples.
    let root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());

    // min_saule_version: refuse to run on a stale toolchain. The comparator
    // lives in `saule-version` so this check, `Saule.atLeast` in the language,
    // and the release tooling can't drift apart.
    if let Some(min) = config.min_saule_version.as_deref()
        && !saule_version::at_least(min)
    {
        eprintln!(
            "error: this project requires Saule {min} or newer (current: {})",
            saule_version::FULL
        );
        process::exit(1);
    }

    let src_dirs: Vec<PathBuf> = config.src_dirs.iter().map(|s| root.join(s)).collect();

    // Resolve each `dependencies:` entry by reading the target project's
    // own `saule.config`. Failures here are fatal because user code that
    // imports a missing dep should not silently fall through to a generic
    // "module not found" error.
    let dependencies = match resolve_dependencies(&root, &config.dependencies) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    saule_interpreter::project::set(saule_interpreter::project::ProjectInfo {
        name: config.name.clone().unwrap_or_default(),
        version: config.version.clone().unwrap_or_default(),
        root: root.clone(),
        src_dirs: src_dirs.clone(),
        dependencies,
    });

    let entry = if is_library {
        None
    } else {
        let entry_rel = config
            .entry
            .clone()
            .unwrap_or_else(|| "src/main.sau".to_string());
        let entry_path = root.join(&entry_rel);
        // Only fatal for `run`. `check` still has every source file to work
        // through, and a missing entry is one of the things it should report
        // rather than die on.
        if !entry_path.is_file() {
            if require_entry {
                eprintln!(
                    "error: entry `{entry_rel}` (from saule.config) does not exist at `{}`",
                    entry_path.display()
                );
                process::exit(1);
            }
            None
        } else {
            Some(entry_path)
        }
    };

    // See `Project::src_dirs` for why this default is applied here and not to
    // the `ProjectInfo` above.
    let walk_dirs = if src_dirs.is_empty() {
        vec![root.join("src")]
    } else {
        src_dirs
    };

    Project {
        src_dirs: walk_dirs,
        entry,
    }
}

/// Parsed `saule.config`. Unknown keys are silently dropped; the format is
/// deliberately minimal — `key: "value"` per line, plus `key: ["a", "b"]`
/// for list-valued keys, plus `--` line comments and blank lines.
#[derive(Debug, Default)]
struct RawConfig {
    name: Option<String>,
    version: Option<String>,
    entry: Option<String>,
    /// `kind: "app"` (default) or `"library"`. A library has no entry point
    /// — it exists to be imported by other projects.
    kind: Option<String>,
    src_dirs: Vec<String>,
    min_saule_version: Option<String>,
    /// Raw `dependencies:` entries — paths (absolute, `~`-expanded, or
    /// relative to the project root) to other Saule projects.
    dependencies: Vec<String>,
}

fn read_config(path: &Path) -> std::io::Result<RawConfig> {
    let text = fs::read_to_string(path)?;
    let mut out = RawConfig::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("--") {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "name" => out.name = Some(unquote(value)),
            "version" => out.version = Some(unquote(value)),
            "entry" => out.entry = Some(unquote(value)),
            "kind" => out.kind = Some(unquote(value)),
            "src_dirs" => out.src_dirs = parse_list(value),
            "min_saule_version" => out.min_saule_version = Some(unquote(value)),
            "dependencies" => out.dependencies = parse_list(value),
            _ => {}
        }
    }
    Ok(out)
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

/// Expand a leading `~` to the user's home directory; pass anything else
/// through unchanged.
fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
}

/// For each `dependencies:` path, locate the dep's `saule.config`, read its
/// `name` + `src_dirs`, and produce a resolved [`saule_interpreter::project::Dependency`].
fn resolve_dependencies(
    project_root: &Path,
    deps: &[String],
) -> Result<Vec<saule_interpreter::project::Dependency>, String> {
    let mut out = Vec::with_capacity(deps.len());
    for raw in deps {
        let expanded = expand_tilde(raw);
        // Relative paths are resolved against the project root, not the cwd,
        // so `saule run` works the same from any directory.
        let dep_root = if expanded.is_absolute() {
            expanded
        } else {
            project_root.join(expanded)
        };
        let dep_root = dep_root
            .canonicalize()
            .map_err(|e| format!("dependency `{raw}`: {e}"))?;

        let dep_config_path = dep_root.join("saule.config");
        if !dep_config_path.is_file() {
            return Err(format!(
                "dependency `{raw}` at `{}` has no `saule.config`",
                dep_root.display()
            ));
        }
        let dep_config = read_config(&dep_config_path)
            .map_err(|e| format!("reading {}: {e}", dep_config_path.display()))?;

        // Name defaults to the dep's directory name so a config that omits
        // `name:` still produces a usable import prefix.
        let name = dep_config
            .name
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                dep_root
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });

        let src_dirs: Vec<PathBuf> = if dep_config.src_dirs.is_empty() {
            vec![dep_root.join("src")]
        } else {
            dep_config
                .src_dirs
                .iter()
                .map(|s| dep_root.join(s))
                .collect()
        };

        out.push(saule_interpreter::project::Dependency {
            name,
            root: dep_root,
            src_dirs,
        });
    }
    Ok(out)
}

/// Parse `["a", "b", "c"]` into `["a", "b", "c"]`. Tolerates missing
/// brackets (treats the value as a single entry) and stray whitespace.
fn parse_list(raw: &str) -> Vec<String> {
    let inner = raw.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|p| unquote(p.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

// Version comparison lives in `saule_version::version_at_least` — one
// comparator for `min_saule_version`, `Saule.atLeast`, and the release
// tooling, so a project that runs cannot be one the language disagrees about.
