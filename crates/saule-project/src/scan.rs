//! Finding a project on disk, and finding the source files inside it.

use std::fs;
use std::path::{Path, PathBuf};

/// The two extensions a Saule source file may have. Both are accepted
/// everywhere `import` resolution accepts them, which is the only reason
/// `.saule` exists at all.
pub const SOURCE_EXTENSIONS: [&str; 2] = ["sau", "saule"];

/// Directory names never descended into.
///
/// The one that matters in practice is `.claude/`: a Claude Code worktree
/// under it is a complete second copy of the repository, and walking into it
/// means analysing — and publishing diagnostics for — every file twice,
/// against stale duplicates the user is not editing. `target/` and
/// `node_modules/` are the same problem with different provenance.
fn is_skipped_dir(name: &str) -> bool {
    name.starts_with('.') || name == "target" || name == "node_modules"
}

/// Whether `path` is a Saule source file by extension.
pub fn is_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SOURCE_EXTENSIONS.contains(&e))
}

/// Walk up from `start` to the nearest ancestor containing a
/// `saule.config`. `start` itself counts.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(crate::CONFIG_FILE).is_file())
        .map(Path::to_path_buf)
}

/// Every source file under `root`, canonicalised, sorted and deduplicated.
///
/// A missing or unreadable directory yields nothing rather than failing:
/// both callers walk directories a config named, and a config naming a
/// directory that does not exist yet is a thing to report elsewhere, not to
/// crash a file walk over.
pub fn scan_sources(root: &Path) -> Vec<PathBuf> {
    scan_all(std::slice::from_ref(&root))
}

/// [`scan_sources`] over several roots at once, with the union sorted and
/// deduplicated — overlapping `src_dirs:` entries must not produce the same
/// file twice.
pub fn scan_all(roots: &[impl AsRef<Path>]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        walk(root.as_ref(), &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !is_skipped_dir(name) {
                walk(&path, out);
            }
        } else if is_source_file(&path) {
            out.push(path.canonicalize().unwrap_or(path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Scratch;

    fn names(root: &Path) -> Vec<String> {
        scan_sources(root)
            .iter()
            .filter_map(|p| p.file_name().and_then(|s| s.to_str()))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn skips_dot_directories() {
        let s = Scratch::new("dotdirs");
        s.write("src/real.sau", "");
        s.write(".claude/worktrees/adoring-cerf/tests/ui/duplicate.sau", "");
        s.write(".git/hooks/hook.sau", "");
        s.write(".idea/scratch.sau", "");

        assert_eq!(names(&s.root()), ["real.sau"]);
    }

    #[test]
    fn skips_build_output_directories() {
        let s = Scratch::new("buildout");
        s.write("src/real.sau", "");
        s.write("target/release/generated.sau", "");
        s.write("node_modules/some-pkg/bundled.sau", "");

        assert_eq!(names(&s.root()), ["real.sau"]);
    }

    #[test]
    fn finds_nested_files_and_both_extensions() {
        let s = Scratch::new("nested");
        s.write("top.sau", "");
        s.write("a/b/c/deep.saule", "");

        assert_eq!(names(&s.root()), ["deep.saule", "top.sau"]);
    }

    #[test]
    fn ignores_files_that_are_not_saule_sources() {
        let s = Scratch::new("exts");
        s.write("keep.sau", "");
        s.write("README.md", "");
        s.write("build.rs", "");
        s.write("saule.config", "");

        assert_eq!(names(&s.root()), ["keep.sau"]);
    }

    /// The skip check is per directory component, not just the top level.
    #[test]
    fn skips_dot_directories_below_the_root() {
        let s = Scratch::new("deepdot");
        s.write("packages/app/src/real.sau", "");
        s.write("packages/app/.claude/worktrees/copy/duplicate.sau", "");

        assert_eq!(names(&s.root()), ["real.sau"]);
    }

    #[test]
    fn missing_root_yields_nothing_rather_than_panicking() {
        let s = Scratch::new("missing");
        assert!(scan_sources(&s.root().join("does-not-exist")).is_empty());
    }

    /// Overlapping source roots are a legal, if odd, config. Producing the
    /// same file twice would make `saule check` report every diagnostic in
    /// it twice.
    #[test]
    fn overlapping_roots_yield_each_file_once() {
        let s = Scratch::new("overlap");
        s.write("src/a.sau", "");
        s.write("src/nested/b.sau", "");

        let both = scan_all(&[s.root().join("src"), s.root().join("src/nested")]);
        assert_eq!(both.len(), 2, "{both:?}");
    }

    #[test]
    fn find_root_walks_up_and_stops_at_the_nearest_config() {
        let s = Scratch::new("findroot");
        s.write("saule.config", "name: \"outer\"");
        s.write("libs/inner/saule.config", "name: \"inner\"");
        s.write("libs/inner/src/deep/x.sau", "");

        assert_eq!(
            find_root(&s.root().join("libs/inner/src/deep")),
            Some(s.root().join("libs/inner"))
        );
        assert_eq!(find_root(&s.root().join("elsewhere")), Some(s.root()));
    }

    #[test]
    fn find_root_returns_none_outside_any_project() {
        let s = Scratch::new("noroot");
        s.write("src/x.sau", "");
        // A scratch dir under the system temp directory has no `saule.config`
        // in any ancestor unless the machine's root does, which would be a
        // stranger problem than this test.
        assert_eq!(find_root(&s.root()), None);
    }
}
