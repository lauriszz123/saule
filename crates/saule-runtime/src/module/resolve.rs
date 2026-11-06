//! Turning an import path into a file on disk: relative paths,
//! project roots, package directories, and folder modules (`init.sau`).

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
        // A file that belongs to a dependency resolves against *its own*
        // package first. Without this, a package cannot have internal
        // folders at all: one of its files reaching another through
        // `core.Color` fails, because the project fallback below searches
        // the *consuming* project's `src_dirs` rather than the dep's, and
        // the dependency lookup after it only fires when the first segment
        // happens to name a dependency.
        //
        // It has to come before that fallback rather than after, or a
        // same-named module in the consumer's tree would satisfy a
        // dependency's private import — `core/Color.sau` in an app would
        // quietly shadow the one inside the library.
        if let Some(dep) = owning_dependency(&info, dir) {
            for src_dir in &dep.src_dirs {
                if let Some(hit) = try_resolve_base(&src_dir.join(&normalised)) {
                    return Some(hit);
                }
            }
        }

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
            // Bare `from "json" import X` — the import names the dependency
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

/// Why `import … from "<raw>"` does not resolve, in the words every tool
/// reports it with — `saule run`, `saule check`, the program driver and the
/// editor all call this, so one missing module reads the same everywhere.
///
/// When an installed library claims the name but could not be used — built
/// against another ABI, installed the old way, not a Saule package — that is
/// the answer, rather than a claim that nothing is there.
pub fn unresolved_import_message(raw: &str) -> String {
    match crate::dynamic_packages::rejection(raw) {
        Some(why) => format!("`{raw}` cannot be imported: {why}"),
        None => format!(
            "could not find module `{raw}` (looked for `.sau` / `.saule` / `init.sau`, \
             a project dependency, and an installed native package)"
        ),
    }
}

/// Every `import` in `module` that does not resolve from `dir`, as errors
/// anchored at the `import` statement, in source order.
///
/// Worth running before anything type-checks the module: an import that
/// does not resolve leaves every name it would have bound unknown, and the
/// first thing to notice is otherwise a use of one of those names, far from
/// the line that actually needs fixing.
pub fn unresolved_imports(module: &saule_ast::Module, dir: &Path) -> Vec<crate::RuntimeError> {
    module
        .stmts
        .iter()
        .filter_map(|stmt| {
            let saule_ast::Stmt::Decl(d) = &stmt.value else {
                return None;
            };
            let saule_ast::Decl::Import { path, .. } = &d.value else {
                return None;
            };
            resolve_import_path(dir, path)
                .is_none()
                .then(|| crate::RuntimeError::ImportError {
                    message: unresolved_import_message(path),
                    span: d.span.clone(),
                })
        })
        .chain(unexported_native_imports(module))
        .collect()
}

/// `from "math" import Nope`: a name a built-in package does not export. The
/// wording is the one the program driver uses when it reaches the import.
///
/// A file of the same name resolves ahead of a package, so one found by the
/// caller's walk is not asked about here; this only reads the packages'
/// export lists, which are known without loading anything.
fn unexported_native_imports(module: &saule_ast::Module) -> Vec<crate::RuntimeError> {
    let mut errors = Vec::new();
    for stmt in &module.stmts {
        let saule_ast::Stmt::Decl(d) = &stmt.value else {
            continue;
        };
        let saule_ast::Decl::Import {
            path,
            names: saule_ast::ImportNames::List(names),
            ..
        } = &d.value
        else {
            continue;
        };
        let Some(pkg) = crate::native_packages::lookup(path) else {
            continue;
        };
        for (name, _) in names {
            if !pkg.exports.contains(&name.as_str()) {
                errors.push(crate::RuntimeError::ImportError {
                    message: format!("`{name}` is not exported from `{path}`"),
                    span: d.span.clone(),
                });
            }
        }
    }
    errors
}

/// The dependency whose `src_dirs` contain `dir` — the package the importing
/// file belongs to, when that is not the project itself.
///
/// The deepest matching `src_dir` wins, so a package vendored inside another
/// resolves against the inner one rather than its host.
fn owning_dependency<'a>(
    info: &'a crate::project::ProjectInfo,
    dir: &Path,
) -> Option<&'a crate::project::Dependency> {
    // `dir` comes from a module path that may or may not have been
    // canonicalized, while `src_dirs` are absolute; compare both resolved so
    // a symlinked package directory still matches.
    let here = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());

    let depth = |dep: &crate::project::Dependency| {
        dep.src_dirs
            .iter()
            .filter(|src| {
                let src = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
                here.starts_with(src)
            })
            .map(|src| src.components().count())
            .max()
    };

    info.dependencies
        .iter()
        .filter_map(|dep| depth(dep).map(|d| (d, dep)))
        .max_by_key(|(d, _)| *d)
        .map(|(_, dep)| dep)
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
