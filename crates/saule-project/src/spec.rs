//! What a `dependencies:` entry can be.
//!
//! Two forms share the one list, told apart by a prefix:
//!
//! ```text
//! dependencies: ["../json", "gh:lauriszz123/saule-shine@v0.1.0"]
//! ```
//!
//! A bare entry is a **path** — absolute, `~`-prefixed, or relative to the
//! project root — and means exactly what it always has. A `gh:` entry is a
//! **package**, fetched from GitHub and installed into `SAULE_HOME` where
//! every project shares it.
//!
//! The prefix is why the two can share a list: `foo/bar` is genuinely
//! ambiguous between a subdirectory and an `owner/repo`, and guessing wrong
//! either way is a confusing failure. With the prefix there is nothing to
//! guess, and a path can never become a network fetch by accident.

use std::fmt;

/// The host a package comes from. One so far; an enum because the parse
/// error should say "unknown source `gl:`" rather than "expected `gh:`",
/// and because `SAULE_HOME` is laid out by host already.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    GitHub,
}

impl Host {
    /// The prefix that selects this host in a `dependencies:` entry.
    pub fn prefix(self) -> &'static str {
        match self {
            Host::GitHub => "gh",
        }
    }

    /// The directory name under `packages/`, and what a message calls it.
    pub fn domain(self) -> &'static str {
        match self {
            Host::GitHub => "github.com",
        }
    }

    /// The URL `git` is pointed at.
    pub fn clone_url(self, owner: &str, repo: &str) -> String {
        match self {
            Host::GitHub => format!("https://github.com/{owner}/{repo}"),
        }
    }
}

/// A package to install: where it lives and which commit-ish to take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub host: Host,
    pub owner: String,
    pub repo: String,
    /// A tag, branch or commit — whatever `git` will check out. `None` means
    /// the repository's default branch, resolved at install time and
    /// recorded then, so what is installed is always a known ref.
    pub reference: Option<String>,
}

impl PackageSpec {
    /// `<owner>/<repo>`, which is how a message names a package and how
    /// `saule remove` lets you refer to one.
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// The entry as it is written in `saule.config`.
    pub fn to_entry(&self) -> String {
        match &self.reference {
            Some(r) => format!("{}:{}@{}", self.host.prefix(), self.slug(), r),
            None => format!("{}:{}", self.host.prefix(), self.slug()),
        }
    }

    /// Whether `needle` names this package: the full entry, `owner/repo`, or
    /// the bare repo name. What `saule remove uikit` matches on.
    pub fn matches(&self, needle: &str) -> bool {
        let needle = needle.trim();
        needle == self.to_entry()
            || needle == self.slug()
            || needle == self.repo
            || Dependency::parse(needle).is_ok_and(|d| match d {
                Dependency::Package(other) => {
                    other.host == self.host && other.owner == self.owner && other.repo == self.repo
                }
                Dependency::Path(_) => false,
            })
    }
}

impl fmt::Display for PackageSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_entry())
    }
}

/// One `dependencies:` entry, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dependency {
    /// A local project: the string exactly as written, resolved against the
    /// project root by [`crate::resolve_dependency`].
    Path(String),
    /// A package fetched from a host and installed into `SAULE_HOME`.
    Package(PackageSpec),
}

impl Dependency {
    /// Parse an entry. A path is anything without a recognised `<host>:`
    /// prefix, so this only fails on a prefix that looks like a package
    /// source but is not usable.
    pub fn parse(raw: &str) -> Result<Dependency, String> {
        let raw = raw.trim();

        // A Windows path (`C:\libs\json`) has a colon in the same place a
        // prefix would. One letter is never a host, so it cannot be one.
        let prefix = raw.split_once(':').map(|(p, _)| p).unwrap_or("");
        if prefix.len() < 2 || !prefix.chars().all(|c| c.is_ascii_alphabetic()) {
            return Ok(Dependency::Path(raw.to_string()));
        }

        let (prefix, rest) = raw.split_once(':').expect("checked above");
        let host = match prefix {
            "gh" => Host::GitHub,
            // Not a host we know. A path is still the likelier reading —
            // `http://…` is not a dependency, but neither is it worth
            // refusing to load a project over.
            _ => return Ok(Dependency::Path(raw.to_string())),
        };

        let (slug, reference) = match rest.split_once('@') {
            Some((s, r)) if !r.trim().is_empty() => (s, Some(r.trim().to_string())),
            Some((s, _)) => (s, None),
            None => (rest, None),
        };
        let slug = slug.trim().trim_matches('/');
        let Some((owner, repo)) = slug.split_once('/') else {
            return Err(format!(
                "`{raw}` is missing the repository: write `{prefix}:<owner>/<repo>`"
            ));
        };
        if owner.is_empty() || repo.is_empty() || repo.contains('/') {
            return Err(format!(
                "`{raw}` is not an `<owner>/<repo>` pair: write `{prefix}:<owner>/<repo>`"
            ));
        }

        Ok(Dependency::Package(PackageSpec {
            host,
            owner: owner.to_string(),
            repo: repo.to_string(),
            reference,
        }))
    }

    /// The package, if this entry is one.
    pub fn as_package(&self) -> Option<&PackageSpec> {
        match self {
            Dependency::Package(p) => Some(p),
            Dependency::Path(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(raw: &str) -> PackageSpec {
        match Dependency::parse(raw).expect("parses") {
            Dependency::Package(p) => p,
            Dependency::Path(p) => panic!("`{raw}` parsed as the path `{p}`"),
        }
    }

    fn path(raw: &str) -> String {
        match Dependency::parse(raw).expect("parses") {
            Dependency::Path(p) => p,
            Dependency::Package(p) => panic!("`{raw}` parsed as the package `{p}`"),
        }
    }

    #[test]
    fn a_package_carries_its_owner_repo_and_ref() {
        let p = pkg("gh:lauriszz123/saule-shine@v0.1.0");
        assert_eq!(p.host, Host::GitHub);
        assert_eq!(p.owner, "lauriszz123");
        assert_eq!(p.repo, "saule-shine");
        assert_eq!(p.reference.as_deref(), Some("v0.1.0"));
        assert_eq!(p.to_entry(), "gh:lauriszz123/saule-shine@v0.1.0");
    }

    #[test]
    fn a_package_without_a_ref_means_the_default_branch() {
        let p = pkg("gh:lauriszz123/saule-shine");
        assert_eq!(p.reference, None);
        assert_eq!(p.to_entry(), "gh:lauriszz123/saule-shine");
    }

    /// Every path spelling the format has ever accepted stays a path. This
    /// is the compatibility promise of putting both in one list.
    #[test]
    fn paths_are_still_paths() {
        for raw in [
            "../json",
            "./libs/http",
            "libs/http",
            "~/libs/http",
            "/usr/share/saule/json",
            "vendor/whatever",
        ] {
            assert_eq!(path(raw), raw, "`{raw}` should be a path");
        }
    }

    /// `C:\libs\json` has a colon exactly where a prefix does. A one-letter
    /// prefix is never a host, which is what keeps them apart.
    #[test]
    fn a_windows_path_is_not_a_package() {
        assert_eq!(path(r"C:\libs\json"), r"C:\libs\json");
        assert_eq!(path(r"D:/libs/json"), r"D:/libs/json");
    }

    /// An unknown prefix reads as a path rather than an error: refusing to
    /// load a whole project over one entry a newer toolchain understands is
    /// the wrong trade, and matches how unknown *keys* are treated.
    #[test]
    fn an_unrecognised_prefix_reads_as_a_path() {
        assert_eq!(path("gl:owner/repo"), "gl:owner/repo");
    }

    #[test]
    fn a_package_without_a_repository_is_an_error() {
        for raw in ["gh:lauriszz123", "gh:", "gh:/repo", "gh:owner/"] {
            assert!(Dependency::parse(raw).is_err(), "`{raw}` should not parse");
        }
    }

    #[test]
    fn an_empty_ref_is_the_same_as_none() {
        assert_eq!(pkg("gh:owner/repo@").reference, None);
    }

    #[test]
    fn remove_matches_by_entry_slug_or_bare_name() {
        let p = pkg("gh:lauriszz123/saule-shine@v0.1.0");
        for needle in [
            "gh:lauriszz123/saule-shine@v0.1.0",
            "gh:lauriszz123/saule-shine",
            "gh:lauriszz123/saule-shine@v9.9.9",
            "lauriszz123/saule-shine",
            "saule-shine",
        ] {
            assert!(p.matches(needle), "`{needle}` should match {p}");
        }
        for needle in ["shine", "lauriszz123/other", "../saule-shine"] {
            assert!(!p.matches(needle), "`{needle}` should not match {p}");
        }
    }
}
