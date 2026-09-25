//! Turning `dependencies:` paths into resolved [`Dependency`] entries.
//!
//! A dependency's *import name* comes from its own `saule.config`, not from
//! where it happens to sit on disk. That is what lets two projects vendor
//! the same library at different paths and still write the same `import`,
//! and it is the reason a package manager for Saule needs no central index.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::info::Dependency;

/// Resolve every entry, failing on the first bad one.
///
/// This is what a *run* wants: an import naming a dependency that could not
/// be resolved should say so, not fall through to a generic "module not
/// found" pointing at the import line.
pub fn resolve_dependencies(
    project_root: &Path,
    deps: &[String],
) -> Result<Vec<Dependency>, String> {
    let mut out = Vec::with_capacity(deps.len());
    for raw in deps {
        // `None` is a compiled package: installed and usable, but reached by
        // the loader's own scan rather than through a source root.
        if let Some(dep) = resolve_entry(project_root, raw)? {
            out.push(dep);
        }
    }
    Ok(out)
}

/// Resolve every entry, dropping the ones that fail.
///
/// This is what an *editor* wants: a workspace with one broken dependency
/// path still gets completion and hover for the other seven, and the broken
/// import is reported where the user can see it — on the import line —
/// rather than by the language server declining to load the project.
pub fn resolve_dependencies_lenient(project_root: &Path, deps: &[String]) -> Vec<Dependency> {
    deps.iter()
        .filter_map(|raw| resolve_entry(project_root, raw).ok().flatten())
        .collect()
}

/// Resolve one `dependencies:` entry by reading the target project's own
/// config for its name and source roots.
///
/// A `gh:` entry resolves to its globally installed copy under `SAULE_HOME`
/// instead of to a path in the project — see [`crate::spec`]. A package that
/// turned out to be a *compiled* one contributes no source roots at all: the
/// loader finds it by scanning `native_packages/`, so there is nothing here
/// for imports to resolve against, and [`Ok(None)`] says so.
pub fn resolve_dependency(project_root: &Path, raw: &str) -> Result<Dependency, String> {
    match resolve_entry(project_root, raw)? {
        Some(dep) => Ok(dep),
        None => Err(format!(
            "dependency `{raw}` is a compiled package and has no source roots"
        )),
    }
}

/// [`resolve_dependency`], but a compiled package answers `None` rather than
/// an error — it is installed and working, it just contributes nothing to
/// import resolution.
pub fn resolve_entry(project_root: &Path, raw: &str) -> Result<Option<Dependency>, String> {
    match crate::spec::Dependency::parse(raw)? {
        crate::spec::Dependency::Path(path) => resolve_path(project_root, &path, raw).map(Some),
        crate::spec::Dependency::Package(spec) => {
            let Some(receipt) = crate::home::Receipt::read(&spec) else {
                return Err(format!(
                    "package `{}` is not installed — run `saule install {}`",
                    spec.slug(),
                    spec.to_entry()
                ));
            };
            match receipt.kind {
                // Discovered by the loader from `native_packages/`, not by
                // path. Nothing to add here.
                crate::home::ReceiptKind::Native { .. } => Ok(None),
                crate::home::ReceiptKind::Source => {
                    let root = crate::home::source_package_path(&spec, &receipt.reference);
                    if !root.is_dir() {
                        return Err(format!(
                            "package `{}` is recorded as installed but `{}` is missing — \
                             run `saule install {}`",
                            spec.slug(),
                            root.display(),
                            spec.to_entry()
                        ));
                    }
                    read_project_at(root, raw).map(Some)
                }
            }
        }
    }
}

/// Resolve a path entry against the project root.
fn resolve_path(project_root: &Path, raw_path: &str, raw: &str) -> Result<Dependency, String> {
    let expanded = expand_tilde(raw_path);
    // Relative paths resolve against the project root, not the process's
    // cwd, so `saule run` behaves the same from any directory.
    let dep_root = if expanded.is_absolute() {
        expanded
    } else {
        project_root.join(expanded)
    };
    let dep_root = dep_root
        .canonicalize()
        .map_err(|e| format!("dependency `{raw}`: {e}"))?;
    read_project_at(dep_root, raw)
}

/// Read a resolved directory as a Saule project: its import name and the
/// source roots it exposes.
fn read_project_at(dep_root: PathBuf, raw: &str) -> Result<Dependency, String> {
    let config_path = dep_root.join(crate::CONFIG_FILE);
    if !config_path.is_file() {
        return Err(format!(
            "dependency `{raw}` at `{}` has no `{}`",
            dep_root.display(),
            crate::CONFIG_FILE
        ));
    }
    let config = Config::read(&config_path)
        .map_err(|e| format!("reading {}: {e}", config_path.display()))?;

    Ok(Dependency {
        name: config.name_or_dir(&dep_root),
        src_dirs: crate::src_dirs_or_default(&dep_root, &config),
        root: dep_root,
    })
}

/// Expand a leading `~/`; pass anything else through unchanged.
pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Scratch;

    #[test]
    fn a_dependency_is_named_by_its_own_config_not_its_path() {
        let s = Scratch::new("dep-name");
        s.write("saule.config", "dependencies: [\"vendor/whatever\"]");
        s.write("vendor/whatever/saule.config", "name: \"json\"");
        s.write("vendor/whatever/src/init.sau", "");

        let deps = resolve_dependencies(&s.root(), &["vendor/whatever".into()]).expect("resolve");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "json");
        assert_eq!(deps[0].src_dirs, [s.canonical("vendor/whatever/src")]);
    }

    #[test]
    fn an_unnamed_dependency_falls_back_to_its_directory() {
        let s = Scratch::new("dep-unnamed");
        s.write("libs/http/saule.config", "-- no name");
        let deps = resolve_dependencies(&s.root(), &["libs/http".into()]).expect("resolve");
        assert_eq!(deps[0].name, "http");
    }

    #[test]
    fn a_dependency_declares_its_own_source_roots() {
        let s = Scratch::new("dep-srcdirs");
        s.write("libs/gfx/saule.config", "src_dirs: [\"lib\", \"ext\"]");
        let deps = resolve_dependencies(&s.root(), &["libs/gfx".into()]).expect("resolve");
        assert_eq!(
            deps[0].src_dirs,
            [
                s.canonical("libs/gfx").join("lib"),
                s.canonical("libs/gfx").join("ext")
            ]
        );
    }

    /// The two callers differ only here, and both spellings must keep
    /// working: `run` needs the error, the language server needs the rest
    /// of the workspace.
    #[test]
    fn strict_resolution_fails_where_lenient_skips() {
        let s = Scratch::new("dep-missing");
        s.write("libs/real/saule.config", "name: \"real\"");

        let raw = vec!["libs/real".to_string(), "libs/gone".to_string()];
        assert!(resolve_dependencies(&s.root(), &raw).is_err());

        let lenient = resolve_dependencies_lenient(&s.root(), &raw);
        assert_eq!(lenient.len(), 1);
        assert_eq!(lenient[0].name, "real");
    }

    /// A directory that exists but is not a Saule project is a clearer
    /// error than "module not found" three steps later.
    #[test]
    fn a_directory_without_a_config_is_not_a_dependency() {
        let s = Scratch::new("dep-noconfig");
        s.write("libs/plain/README.md", "");
        let err = resolve_dependencies(&s.root(), &["libs/plain".into()]).unwrap_err();
        assert!(err.contains("saule.config"), "{err}");
    }
}
