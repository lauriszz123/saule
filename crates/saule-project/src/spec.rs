//! What a `dependencies:` entry can be.
//!
//! Two forms share the one list:
//!
//! ```text
//! dependencies: ["../json", "lauriszz123@saule-shine:v0.1.0"]
//! ```
//!
//! A **package** is `owner@repo`, optionally pinned with `:version` — fetched
//! from GitHub and installed into `SAULE_HOME`, where every project shares
//! it. Without a version it means the repository's newest tag, resolved at
//! install time and recorded then.
//!
//! Anything else is a **path** — absolute, `~`-prefixed, or relative to the
//! project root — and means exactly what it always has: used where it lies,
//! never fetched.
//!
//! The `@` is what keeps the two apart, and it is why the separator is not a
//! `/`: `foo/bar` is genuinely ambiguous between a subdirectory and an
//! `owner/repo`, and guessing wrong either way is a confusing failure. An
//! `@` appears in neither a relative path nor a Windows one, so a path can
//! never become a network fetch by accident. A path that does contain one is
//! still writable as `./my@dir`, since a leading `.` is not an owner.
//!
//! `gh:owner/repo@ref` — the spelling this replaced — still parses, so a
//! config written against it keeps working.

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

    /// The entry as it is written in `saule.config`, and as a message spells
    /// this package back to the user: `owner@repo`, with `:version` when a
    /// ref is pinned.
    pub fn to_entry(&self) -> String {
        // One arm on purpose. GitHub is the default host and so needs no
        // prefix — which is what makes `owner@repo:version` the spelling in
        // both the config and the command line. A second host cannot share
        // that unprefixed form without becoming ambiguous, so adding a
        // `Host` variant must come back here and decide how it spells
        // itself; the exhaustive match is what forces that.
        let base = match self.host {
            Host::GitHub => format!("{}@{}", self.owner, self.repo),
        };
        match &self.reference {
            Some(r) => format!("{base}:{r}"),
            None => base,
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
    /// Parse an entry.
    ///
    /// `owner@repo[:version]` is a package. Everything else is a path, except
    /// the legacy `<host>:owner/repo[@ref]` spelling, which is also a
    /// package — so this only fails on something that is clearly trying to
    /// be a package and cannot be read as one.
    pub fn parse(raw: &str) -> Result<Dependency, String> {
        let raw = raw.trim();

        // The current spelling, checked first: it has to win over the `gh:`
        // branch below, which would otherwise read the `:version` of
        // `owner@repo:v1` as a host prefix.
        if let Some(spec) = parse_at_form(raw) {
            return Ok(Dependency::Package(spec));
        }

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

/// Read `owner@repo[:version]`, or `None` if `raw` is not that shape.
///
/// Deliberately strict about the owner: it must be a plain name, which is
/// what keeps every other kind of entry out. `gh:owner/repo@v1` has an owner
/// half of `gh:owner/repo` — a colon and a slash, so not a name — and falls
/// through to the legacy branch. `../my@dir` starts with a `.`, and
/// `C:\libs` has no `@` at all, so both stay paths.
fn parse_at_form(raw: &str) -> Option<PackageSpec> {
    let (owner, rest) = raw.split_once('@')?;
    if !is_owner_name(owner) {
        return None;
    }
    // The *first* colon ends the repo. A tag may contain one, and the repo
    // name may not, so splitting here cannot cut a tag short.
    let (repo, reference) = match rest.split_once(':') {
        Some((repo, r)) if !r.trim().is_empty() => (repo, Some(r.trim().to_string())),
        // `owner@repo:` with nothing after it reads as "no version given"
        // rather than as an error: it is what a half-finished command line
        // looks like, and the newest tag is the useful answer.
        Some((repo, _)) => (repo, None),
        None => (rest, None),
    };
    is_repo_name(repo).then(|| PackageSpec {
        host: Host::GitHub,
        owner: owner.to_string(),
        repo: repo.to_string(),
        reference,
    })
}

/// Whether `s` is a bare owner name: letters, digits, `-` and `_`.
///
/// GitHub itself allows only alphanumerics and hyphens; `_` is accepted
/// because rejecting it here would report "not a package" for what is
/// obviously meant as one, and the fetch reports a missing owner perfectly
/// well on its own.
fn is_owner_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `s` is a bare repository name — an owner name, plus the `.` that
/// plenty of repositories have (`saule.vim`).
fn is_repo_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
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
    fn a_package_carries_its_owner_repo_and_version() {
        let p = pkg("lauriszz123@saule-shine:v0.1.0");
        assert_eq!(p.host, Host::GitHub);
        assert_eq!(p.owner, "lauriszz123");
        assert_eq!(p.repo, "saule-shine");
        assert_eq!(p.reference.as_deref(), Some("v0.1.0"));
        assert_eq!(p.to_entry(), "lauriszz123@saule-shine:v0.1.0");
    }

    /// No `:version` means the newest tag, resolved at install time. The
    /// spec carries `None` and the installer is what turns that into a ref.
    #[test]
    fn a_package_without_a_version_means_the_newest_tag() {
        let p = pkg("lauriszz123@saule-shine");
        assert_eq!(p.reference, None);
        assert_eq!(p.to_entry(), "lauriszz123@saule-shine");
    }

    /// The spelling this replaced. A config written against it still loads,
    /// and is rewritten to the current spelling the next time the package is
    /// installed — which is why both parse to the same thing.
    #[test]
    fn the_legacy_gh_spelling_still_parses() {
        let old = pkg("gh:lauriszz123/saule-shine@v0.1.0");
        let new = pkg("lauriszz123@saule-shine:v0.1.0");
        assert_eq!(old, new);
        assert_eq!(old.to_entry(), "lauriszz123@saule-shine:v0.1.0");

        assert_eq!(
            pkg("gh:lauriszz123/saule-shine"),
            pkg("lauriszz123@saule-shine")
        );
    }

    /// A repository name may contain a dot (`saule.vim`), and a tag may
    /// contain a colon or a slash (`release/1.0`) — the first colon ends the
    /// repo, so neither is cut short.
    #[test]
    fn dots_in_a_repo_and_separators_in_a_version_survive() {
        let p = pkg("owner@saule.vim:v1.0");
        assert_eq!(p.repo, "saule.vim");
        assert_eq!(p.reference.as_deref(), Some("v1.0"));

        assert_eq!(
            pkg("owner@repo:release/1.0").reference.as_deref(),
            Some("release/1.0")
        );
    }

    /// An `@` in a path is the one case the two forms could collide. A
    /// leading `.` is not an owner name, so the escape hatch is `./`.
    #[test]
    fn a_path_containing_an_at_sign_is_still_a_path() {
        for raw in [
            "./my@dir",
            "../cache@2/json",
            "~/libs@old/http",
            r"C:\libs@1\json",
        ] {
            assert_eq!(path(raw), raw, "`{raw}` should be a path");
        }
    }

    /// Neither half may be empty, and an owner is a plain name — so these
    /// are paths, not malformed packages, and loading a project over one
    /// does not fail.
    #[test]
    fn an_at_sign_alone_does_not_make_a_package() {
        for raw in ["@repo", "owner@", "@", "a/b@c", "owner@re/po"] {
            assert_eq!(path(raw), raw, "`{raw}` should be a path");
        }
    }

    /// `owner@repo:` is what a half-typed command line looks like. The
    /// newest tag is a more useful reading than an error.
    #[test]
    fn an_empty_version_means_the_newest_tag() {
        assert_eq!(pkg("owner@repo:").reference, None);
    }

    #[test]
    fn a_package_is_matched_by_either_spelling() {
        let p = pkg("lauriszz123@saule-shine:v0.1.0");
        for needle in [
            "lauriszz123@saule-shine:v0.1.0",
            "lauriszz123@saule-shine",
            "lauriszz123@saule-shine:v9.9.9",
            "gh:lauriszz123/saule-shine@v0.1.0",
            "lauriszz123/saule-shine",
            "saule-shine",
        ] {
            assert!(p.matches(needle), "`{needle}` should match {p}");
        }
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
