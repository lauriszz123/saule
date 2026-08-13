//! Project-mode bootstrap: read `saule.config`, configure the interpreter's
//! project context, then hand off to [`crate::run::run_file`] on the entry
//! point.
//!
//! The format itself, dependency resolution and source scanning live in
//! `saule-project`, shared with the language server. What stays here is the
//! part that is genuinely CLI: deciding which failures are fatal, and saying
//! so on stderr before exiting.

use std::{
    path::{Path, PathBuf},
    process,
};

use saule_project::{Config, Kind, ProjectInfo};

use crate::run::run_file;

/// A project after its `saule.config` has been read and
/// [`saule_project::set`] has been called.
///
/// Produced by [`configure_project`] and consumed by both `run` (which wants
/// the entry point) and `check` (which wants every source file).
pub(crate) struct Project {
    /// Directories holding this project's own sources, in declaration order.
    ///
    /// Not the same list as [`ProjectInfo::src_dirs`] — see
    /// [`saule_project::src_dirs_or_default`] for why the two readings of an
    /// absent `src_dirs:` differ.
    pub src_dirs: Vec<PathBuf>,
    /// Absolute path to `entry:`. `None` for `kind: "library"`, which has no
    /// entry point by definition.
    pub entry: Option<PathBuf>,
}

impl Project {
    /// Every source file under `src_dirs`, sorted and deduplicated.
    ///
    /// `check` walks all of them rather than following imports from the entry
    /// point: a file no one imports yet is exactly the file whose errors you
    /// want to hear about before you import it.
    pub fn source_files(&self) -> Vec<PathBuf> {
        saule_project::scan_all(&self.src_dirs)
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
    let config_path = dir.join(saule_project::CONFIG_FILE);
    if !config_path.exists() {
        eprintln!(
            "error: no `{}` in `{}`\n\nRun `saule init <name>` to create one, or pass a file path.",
            saule_project::CONFIG_FILE,
            dir.display()
        );
        process::exit(1);
    }

    let config = match Config::read(&config_path) {
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
    let kind = match config.kind() {
        Ok(k) => k,
        Err(other) => {
            eprintln!(
                "error: unknown `kind: \"{other}\"` in saule.config — expected \"app\" or \"library\""
            );
            process::exit(1);
        }
    };
    let is_library = kind == Kind::Library;
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

    // Failures resolving a dependency are fatal because user code that
    // imports a missing dep should not silently fall through to a generic
    // "module not found" error.
    let info = match ProjectInfo::resolve(dir, &config) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };
    // `resolve` canonicalised the root, so every `pretty_path` and `src_dirs`
    // comparison downstream compares like with like.
    let root = info.root.clone();
    saule_project::set(info);

    let entry = if is_library {
        None
    } else {
        let entry_rel = config
            .entry
            .clone()
            .unwrap_or_else(|| saule_project::DEFAULT_ENTRY.to_string());
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

    Project {
        src_dirs: saule_project::src_dirs_or_default(&root, &config),
        entry,
    }
}
