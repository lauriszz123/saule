//! Resolve the toolchain version at compile time and bake it in.
//!
//! The scheme is `<two-digit year>.<build number>` — `26.7` is the seventh
//! release cut in 2026. There is no patch component: a fix is just the next
//! build number, which removes the "is this a minor or a patch?" judgement
//! call from every release.
//!
//! Resolution order, first match wins:
//!
//! 1. **`$SAULE_VERSION`** — what CI sets. Authoritative, used verbatim.
//! 2. **Git tags** — the build number is derived from the `v<year>.*` tags
//!    that already exist. When `HEAD` is exactly one of those tags and the
//!    tree is clean, this is that release. Otherwise it's a development
//!    build *heading toward* the next number, so the build number is the
//!    highest released one plus one and `-dev` is appended.
//! 3. **Neither** — building from a source tarball with no `.git`. Build
//!    number `0`, marked dev. `26.0` is deliberately a number no release
//!    ever carries, so "unknown provenance" is never mistaken for a release.
//!
//! The *year* always comes from the major component of the workspace's
//! `version` in `Cargo.toml`. That is the one place the year is written
//! down, and the only thing that has to be touched in January.

use std::process::Command;

fn main() {
    // Without these, Cargo caches the build script's output and a commit or
    // a new tag would not change the baked-in version.
    println!("cargo:rerun-if-env-changed=SAULE_VERSION");
    for path in git_watch_paths() {
        println!("cargo:rerun-if-changed={path}");
    }

    let year = year_from_cargo_version();
    let resolved = resolve(year);

    println!("cargo:rustc-env=SAULE_VERSION_YEAR={}", resolved.year);
    println!("cargo:rustc-env=SAULE_VERSION_BUILD={}", resolved.build);
    println!(
        "cargo:rustc-env=SAULE_VERSION_IS_DEV={}",
        u8::from(resolved.is_dev)
    );
    println!("cargo:rustc-env=SAULE_VERSION_COMMIT={}", resolved.commit);
    println!(
        "cargo:rustc-env=SAULE_VERSION_STR={}.{}",
        resolved.year, resolved.build
    );
    println!("cargo:rustc-env=SAULE_VERSION_FULL={}", resolved.full());
}

struct Resolved {
    year: u32,
    build: u32,
    is_dev: bool,
    /// Short commit hash, or empty when there is no git to ask.
    commit: String,
}

impl Resolved {
    /// The long form: `26.7` for a release, `26.8-dev+1a2b3c4` otherwise.
    /// Only the long form mentions the commit — a release's version string
    /// stays short enough to say out loud.
    fn full(&self) -> String {
        let short = format!("{}.{}", self.year, self.build);
        if !self.is_dev {
            return short;
        }
        if self.commit.is_empty() {
            format!("{short}-dev")
        } else {
            format!("{short}-dev+{}", self.commit)
        }
    }
}

/// The year of record: the major component of the workspace `version`.
fn year_from_cargo_version() -> u32 {
    std::env::var("CARGO_PKG_VERSION")
        .ok()
        .and_then(|v| v.split('.').next().map(str::to_string))
        .and_then(|major| major.parse().ok())
        .unwrap_or(0)
}

fn resolve(year: u32) -> Resolved {
    // 1. CI hands us the answer.
    if let Some(explicit) = std::env::var("SAULE_VERSION")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        let is_dev = explicit.contains("-dev");
        let (y, b) = parse_version(&explicit).unwrap_or((year, 0));
        return Resolved {
            year: y,
            build: b,
            is_dev,
            commit: git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_default(),
        };
    }

    // 2. Ask git.
    let Some(commit) = git(&["rev-parse", "--short=7", "HEAD"]) else {
        // 3. No git at all.
        return Resolved {
            year,
            build: 0,
            is_dev: true,
            commit: String::new(),
        };
    };

    let released = highest_released_build(year);

    // A clean tree sitting exactly on `v<year>.<n>` *is* that release —
    // this is what makes a locally built tag identical to the CI artifact.
    let tree_is_clean = git(&["status", "--porcelain"]).is_some_and(|s| s.trim().is_empty());
    if tree_is_clean && let Some(build) = tagged_build_at_head(year) {
        return Resolved {
            year,
            build,
            is_dev: false,
            commit,
        };
    }

    Resolved {
        year,
        // Heading toward the next release, not sitting on the last one.
        build: released.unwrap_or(0) + 1,
        is_dev: true,
        commit,
    }
}

/// Highest `n` across all existing `v<year>.<n>` tags, or `None` when this
/// year has no releases yet.
fn highest_released_build(year: u32) -> Option<u32> {
    let out = git(&["tag", "--list", &format!("v{year}.*")])?;
    out.lines()
        .filter_map(parse_version)
        .filter(|(y, _)| *y == year)
        .map(|(_, b)| b)
        .max()
}

/// The build number of a `v<year>.<n>` tag pointing at `HEAD`, if any.
fn tagged_build_at_head(year: u32) -> Option<u32> {
    let out = git(&["tag", "--points-at", "HEAD"])?;
    out.lines()
        .filter_map(parse_version)
        .filter(|(y, _)| *y == year)
        .map(|(_, b)| b)
        .max()
}

/// Parse `v26.7`, `26.7`, or `26.7-dev+abc` into `(26, 7)`. Anything that
/// isn't two leading numeric components is rejected rather than guessed at,
/// so an unrelated tag in the repo can never be read as a version.
fn parse_version(raw: &str) -> Option<(u32, u32)> {
    let raw = raw.trim();
    let raw = raw.strip_prefix('v').unwrap_or(raw);
    let mut parts = raw.split('.');
    let year: u32 = parts.next()?.parse().ok()?;
    let build_part = parts.next()?;
    let digits: String = build_part
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    Some((year, digits.parse().ok()?))
}

/// Run git in the crate's directory, returning trimmed stdout on success.
/// Every failure mode — git missing, not a repository, command error —
/// collapses to `None` so a build never depends on git being present.
fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}

/// Files whose contents change when HEAD moves or a tag is created. Watched
/// only if they exist — naming a missing path would make Cargo rebuild this
/// crate on every single invocation.
fn git_watch_paths() -> Vec<String> {
    let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return Vec::new();
    };
    let Some(root) = git(&["rev-parse", "--git-dir"]) else {
        return Vec::new();
    };
    // `--git-dir` is relative to the crate directory when it resolves inside
    // the same worktree, absolute otherwise.
    let git_dir = if std::path::Path::new(&root).is_absolute() {
        std::path::PathBuf::from(root)
    } else {
        std::path::PathBuf::from(&dir).join(root)
    };
    ["HEAD", "packed-refs", "refs/tags"]
        .iter()
        .map(|p| git_dir.join(p))
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect()
}
