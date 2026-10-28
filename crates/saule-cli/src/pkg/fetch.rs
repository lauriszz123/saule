//! Getting a package's source onto disk.
//!
//! Through `git`, which is a hard dependency of `saule install` and of
//! nothing else — `saule run` never touches it. That is the cost of having
//! no package index, and a fair one: anyone consuming a git-hosted package
//! has git.

use std::path::{Path, PathBuf};
use std::process::Command;

use saule_project::spec::PackageSpec;

/// A fetched package: where it landed, and the ref it actually is.
pub(crate) struct Fetched {
    pub(crate) dir: PathBuf,
    /// The ref as asked for, or the default branch resolved to its name, so
    /// what was installed is always recorded as something specific.
    pub(crate) reference: String,
}

/// Clone `spec` into a fresh directory under `parent`.
///
/// Shallow and single-branch: a package is wanted at one commit, and the
/// history behind it is bytes nobody reads.
pub(crate) fn clone(spec: &PackageSpec, parent: &Path) -> Result<Fetched, String> {
    let url = spec.host.clone_url(&spec.owner, &spec.repo);
    let dir = parent.join(&spec.repo);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;

    let mut args = vec![
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--quiet".to_string(),
    ];
    if let Some(reference) = &spec.reference {
        args.push("--branch".to_string());
        args.push(reference.clone());
    }
    args.push(url.clone());
    args.push(dir.to_string_lossy().into_owned());

    let out = Command::new("git")
        .args(&args)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "`git` is not installed, and `saule install` needs it to fetch a package".into()
            }
            _ => format!("could not run `git`: {e}"),
        })?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        // `--branch` takes a tag or a branch but not a commit, and the
        // message git gives for a missing one does not say which was meant.
        if let Some(reference) = &spec.reference
            && err.contains("not found in upstream origin")
        {
            return Err(format!(
                "`{}` has no tag or branch `{reference}`\n  \
                 `saule install` takes a tag or a branch; check the repository's releases",
                spec.slug()
            ));
        }
        return Err(format!("could not fetch `{url}`:\n  {err}"));
    }

    let reference = match &spec.reference {
        Some(r) => r.clone(),
        None => default_branch(&dir)?,
    };
    Ok(Fetched { dir, reference })
}

/// The branch a fresh clone landed on, so an install with no ref records
/// what it actually took rather than "whatever was current".
fn default_branch(dir: &Path) -> Result<String, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("could not run `git`: {e}"))?;
    if !out.status.success() {
        return Ok("HEAD".to_string());
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(if name.is_empty() { "HEAD".into() } else { name })
}

/// The commit a fetched tree is at. Shown when installing, because a branch
/// name alone does not say what you got.
pub(crate) fn short_commit(dir: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

// ─── what a repository offers ───────────────────────────────────────────────

/// Run `git` with `args`, returning stdout on success.
///
/// The "git is missing" case is reported the same way wherever it happens,
/// because it is the one failure here that is about the machine rather than
/// about the package.
fn git(args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "`git` is not installed, and `saule install` needs it to reach a package".into()
            }
            _ => format!("could not run `git`: {e}"),
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "git failed without saying why".to_string()
        } else {
            err
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Every tag a repository has, newest first.
///
/// Asked over the network without cloning, which is what makes resolving
/// "the newest version" cheap enough to do on every `saule install` and
/// every `saule update --check`.
///
/// Ordered by [`saule_version::compare`] — the same comparator behind
/// `min_saule_version` and `Saule.atLeast`. That matters more than it looks:
/// sorted as strings, `v1.10.0` comes *before* `v1.9.0`, so a project would
/// silently stop updating after its tenth release.
pub(crate) fn tags(spec: &PackageSpec) -> Result<Vec<String>, String> {
    let url = spec.host.clone_url(&spec.owner, &spec.repo);
    // `--refs` drops the peeled `refs/tags/x^{}` lines an annotated tag adds,
    // which would otherwise each arrive as a second copy of the same tag.
    let out = git(&["ls-remote", "--tags", "--refs", &url])
        .map_err(|e| format!("could not read the tags of `{}`:\n  {e}", spec.slug()))?;

    let mut tags: Vec<String> = out
        .lines()
        .filter_map(|line| line.split_once('\t').map(|(_, r)| r))
        .filter_map(|r| r.strip_prefix("refs/tags/"))
        .map(str::to_string)
        .collect();
    // Newest first. The lexicographic tiebreak matters only for tags that
    // compare numerically equal — `v1.0.0-rc1` against `v1.0.0-rc2`, which
    // `compare` counts as the same version — and it keeps that order
    // deterministic instead of down to the remote's listing.
    tags.sort_by(|a, b| saule_version::compare(b, a).then_with(|| b.cmp(a)));
    Ok(tags)
}

/// The newest tag, or `None` for a repository that has none.
///
/// Pre-releases are passed over: `v3.0.0-rc1` is a newer *version* than
/// `v2.0.0` and [`saule_version::compare`] rightly says so, but it is not the
/// newest *release*, and handing a release candidate to someone who typed
/// `saule install acme@uikit` is not what they asked for. Pinning still
/// reaches one — `acme@uikit:v3.0.0-rc1` installs exactly that.
///
/// A repository holding nothing but pre-releases falls back to the newest of
/// them, since installing nothing at all would be the worse answer.
///
/// `None` is not an error: plenty of packages are developed on a branch and
/// have never cut a release, and the caller falls back to the default branch
/// for them.
pub(crate) fn latest_tag(spec: &PackageSpec) -> Result<Option<String>, String> {
    let tags = tags(spec)?;
    Ok(tags
        .iter()
        .find(|t| !saule_version::is_prerelease(t))
        .or_else(|| tags.first())
        .cloned())
}

/// The commit a ref points at on the remote, without cloning.
///
/// What answers "has this moved?" for a package installed from a *branch*,
/// where the ref name alone never changes. `None` means the ref is gone, or
/// that git could not be reached — both are "nothing to compare", and an
/// update check that cannot reach the network should say so rather than
/// report everything as current.
pub(crate) fn remote_commit(spec: &PackageSpec, reference: &str) -> Option<String> {
    let url = spec.host.clone_url(&spec.owner, &spec.repo);
    let out = git(&["ls-remote", &url, reference]).ok()?;
    let sha = out.lines().next()?.split_once('\t')?.0.trim();
    (!sha.is_empty()).then(|| sha.to_string())
}
