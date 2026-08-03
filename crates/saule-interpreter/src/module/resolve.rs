//! Turning an import path into a file on disk: relative paths,
//! project roots, package directories, and `init.sau` barrels.

use std::path::{Path, PathBuf};

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
    // Native package wins over any filesystem lookup so a registered
    // `"io"` package can never be shadowed by a stray `io.sau` next to
    // the importing file.
    if crate::native_packages::lookup(raw).is_some() {
        return Some(crate::native_packages::sentinel_path(raw));
    }

    // Dynamically-discovered native packages (manifest-described shared
    // libraries under `~/.saule/`) resolve next, before any filesystem
    // lookup, so an installed `"engine"` package can't be shadowed by a
    // stray `engine.sau`.
    if crate::dynamic_packages::is_dynamic_package(raw) {
        return Some(crate::dynamic_packages::sentinel_path(raw));
    }

    let normalised = raw.replace('.', "/");

    if let Some(hit) = try_resolve_base(&dir.join(&normalised)) {
        return Some(hit);
    }

    // Project-wide `src_dirs:` fallback. These are absolute, so we look the
    // import up under each in turn before giving up.
    if let Some(info) = crate::project::get() {
        for src_dir in &info.src_dirs {
            if let Some(hit) = try_resolve_base(&src_dir.join(&normalised)) {
                return Some(hit);
            }
        }

        // Dependency lookup: if the first path segment names a dep, strip
        // it and resolve the remainder under that dep's `src_dirs`.
        if let Some((head, rest)) = normalised.split_once('/') {
            for dep in &info.dependencies {
                if dep.name == head {
                    for src_dir in &dep.src_dirs {
                        if let Some(hit) = try_resolve_base(&src_dir.join(rest)) {
                            return Some(hit);
                        }
                    }
                }
            }
        } else {
            // Bare `import X from "json"` — the import names the dependency
            // itself rather than a module inside it, so it resolves to the
            // package's entry point: an `init.sau` under one of its
            // `src_dirs`. That is the single convention for "what a package
            // exposes"; see `try_resolve_base`.
            for dep in &info.dependencies {
                if dep.name == normalised {
                    for src_dir in &dep.src_dirs {
                        if let Some(hit) = try_resolve_base(src_dir) {
                            return Some(hit);
                        }
                    }
                }
            }
        }
    }

    None
}

pub(crate) fn try_resolve_base(base: &Path) -> Option<PathBuf> {
    let candidates = [
        base.with_extension("sau"),
        base.with_extension("saule"),
        base.join("init.sau"),
        base.join("init.saule"),
        base.to_path_buf(),
    ];

    for candidate in candidates {
        if candidate.is_file() {
            return std::fs::canonicalize(&candidate).ok().or(Some(candidate));
        }
    }
    None
}

/// True when `path` is a folder module's entry file (`init.sau` /
/// `init.saule`).
///
/// Such a file is a *barrel*: it re-exports whatever it imports, so a folder
/// of files can be consumed as one module. See [`collect_exports`].
pub(crate) fn is_init_module(path: &Path) -> bool {
    path.file_stem().and_then(|s| s.to_str()) == Some("init")
}
