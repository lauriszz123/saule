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
