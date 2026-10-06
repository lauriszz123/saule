//! The Saule toolchain's version, resolved once at compile time by
//! [`build.rs`](../build.rs) and shared by every crate in the workspace.
//!
//! # The scheme
//!
//! ```text
//! 26.7
//! ^^ ^
//! |  └── build number, counting up from 1 within the year
//! └───── two-digit year
//! ```
//!
//! Two components, both meaningful. There is no patch component, because a
//! patch would need someone to decide whether a change is "minor" or
//! "patch" — a judgement that costs real time and communicates almost
//! nothing to the person installing the toolchain. `26.7` came after `26.6`
//! and that is the whole story.
//!
//! Build numbers **reset each year** and versions still compare correctly,
//! because the year is the leading component: `27.1` > `26.412` by ordinary
//! numeric comparison.
//!
//! # Which constant to use
//!
//! * [`VERSION`] — `"26.7"`. The version *as a version*: what a user types
//!   into `min_saule_version`, what a comparison is run against, what a
//!   release artifact is named. A development build reports the number it
//!   is heading toward, so building against an unreleased feature and then
//!   declaring `min_saule_version` for it works.
//! * [`FULL`] — `"26.7"` for a release, `"26.8-dev+1a2b3c4"` otherwise.
//!   For anything a human reads: `--version`, bug reports, LSP handshakes.
//!   Never parse it; parse [`VERSION`].
//! * [`IS_DEV`] — whether this build came from a tagged, clean tree.

use std::cmp::Ordering;

/// `"26.7"` — year and build number, nothing else. Stable enough to parse.
pub const VERSION: &str = env!("SAULE_VERSION_STR");

/// `"26.7"` for a release, `"26.8-dev+1a2b3c4"` for anything else. For
/// display only.
pub const FULL: &str = env!("SAULE_VERSION_FULL");

/// Two-digit year: `26`.
pub const YEAR: u32 = parse_u32(env!("SAULE_VERSION_YEAR"));

/// Build number within [`YEAR`], counting from 1. `0` means the version
/// could not be determined — a source tree with no git and no
/// `$SAULE_VERSION` — and is never carried by a real release.
pub const BUILD: u32 = parse_u32(env!("SAULE_VERSION_BUILD"));

/// `false` only for a build made from a clean tree sitting exactly on a
/// release tag. Everything else — local work, CI on a branch, a dirty tree
/// on a tag — is a development build.
pub const IS_DEV: bool = parse_u32(env!("SAULE_VERSION_IS_DEV")) == 1;

/// Short commit hash this was built from, or `""` when git was unavailable.
pub const COMMIT: &str = env!("SAULE_VERSION_COMMIT");

// Invariants of the scheme itself. Asserted at compile time rather than in a
// test, because a build that violates one has already produced binaries that
// misreport their version — there is no useful "test failed" state to reach.
const _: () = assert!(
    YEAR >= 26,
    "the year of record in Cargo.toml must be a two-digit year, 26 or later"
);
const _: () = assert!(
    BUILD > 0 || IS_DEV,
    "build 0 is the couldn't-determine marker and must never be a release"
);

/// Is this toolchain at least `required`?
///
/// Compares dotted numeric components left to right, treating a missing
/// component as `0` — so `"26.7"` satisfies `"26"`, `"26.7"`, and `"26.0"`,
/// but not `"26.8"` or `"27.1"`. Non-numeric trailing text on a component
/// is ignored (`"26.8-dev"` compares as `26.8`), which is what makes a
/// development build usable against a `min_saule_version` naming the
/// release it is heading toward.
///
/// This is the comparator behind `min_saule_version` in `saule.config` and
/// `Saule.atLeast` in the language, so all three agree by construction.
pub fn at_least(required: &str) -> bool {
    version_at_least(VERSION, required)
}

/// [`at_least`] with an explicit left-hand side. Separate so it can be
/// tested against versions this build doesn't happen to have.
pub fn version_at_least(current: &str, required: &str) -> bool {
    compare(current, required) != Ordering::Less
}

/// Order two dotted version strings.
///
/// The one comparator for every version-shaped string the toolchain reads:
/// `min_saule_version` and `Saule.atLeast` ask it a `>=` question through
/// [`version_at_least`], and `saule install` / `saule update` sort a
/// repository's git tags with it to decide which one is newest. Keeping the
/// two uses on one implementation is the point — a package manager that
/// disagreed with the language about whether `v26.10` beats `v26.9` would be
/// a long afternoon.
///
/// Rules, in order:
///
/// * A leading `v` is ignored, so `v1.2.0` and `1.2.0` are the same version.
/// * Dotted components compare numerically, left to right, and a missing
///   component is `0` — so `1.2` equals `1.2.0`, and `1.10` beats `1.9`
///   (which a string comparison gets backwards).
/// * Non-numeric trailing text on a component is ignored (`26.8-dev` compares
///   as `26.8`), which is what lets a development build satisfy a
///   `min_saule_version` naming the release it is heading toward.
///
/// So this is a comparison of numbers and nothing else, and in particular
/// `1.0.0-rc1` and `1.0.0` compare **equal**. That is deliberate, and it is
/// why there is a separate [`is_prerelease`]: ranking a pre-release below its
/// release here would read naturally but would also say `26.8-dev` is short
/// of `26.8`, which would stop every development build from satisfying its own
/// `min_saule_version`. Picking a release over a candidate is a policy
/// question, and `saule install` answers it by filtering with
/// [`is_prerelease`] rather than by sorting.
pub fn compare(a: &str, b: &str) -> Ordering {
    let left = components(a);
    let right = components(b);
    for i in 0..left.len().max(right.len()) {
        let x = left.get(i).copied().unwrap_or(0);
        let y = right.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    Ordering::Equal
}

/// The dotted numeric components of a version, `v` stripped and non-digit
/// trailing text on each component dropped.
fn components(v: &str) -> Vec<u32> {
    let v = v.trim();
    v.strip_prefix('v')
        .unwrap_or(v)
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

/// Whether a version carries a pre-release suffix — `-rc1`, `-beta.2`.
///
/// Only a `-` counts. A `+build` suffix is metadata that semver explicitly
/// excludes from precedence, and a bare trailing letter (`1.2.0a`) is too
/// rare a spelling to guess at.
///
/// Public because [`compare`] is the wrong place to act on it: ordering and
/// *policy* are different questions. `v3.0.0-rc1` really is a newer version
/// than `v2.0.0`, so `compare` says so — while `saule install` consults this
/// to pass over a release candidate that nobody asked for.
pub fn is_prerelease(v: &str) -> bool {
    let v = v.trim();
    v.strip_prefix('v').unwrap_or(v).contains('-')
}

/// Digits-only `const` parse, so the numeric constants above stay `const`.
/// The inputs come from our own build script, so a non-digit is a bug here
/// rather than bad user input — hence the panic at compile time.
const fn parse_u32(s: &str) -> u32 {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut n = 0u32;
    while i < bytes.len() {
        let d = bytes[i];
        assert!(
            d.is_ascii_digit(),
            "build script emitted a non-numeric value"
        );
        n = n * 10 + (d - b'0') as u32;
        i += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    // The `YEAR >= 26` and "build 0 is never a release" invariants are
    // asserted at compile time above, so they have no test here.

    /// The ordering `saule install` sorts git tags with. The numeric case is
    /// the one a string sort gets wrong, and getting it wrong means a package
    /// silently stops updating after its ninth release.
    #[test]
    fn compare_orders_versions_numerically() {
        for (older, newer) in [
            ("1.9.0", "1.10.0"),
            ("v1.9.0", "v1.10.0"),
            ("26.9", "26.412"),
            ("26.412", "27.1"),
            ("1.2.3", "1.3.0"),
            ("0.1.0", "1.0.0"),
        ] {
            assert_eq!(compare(older, newer), Ordering::Less, "{older} < {newer}");
            assert_eq!(
                compare(newer, older),
                Ordering::Greater,
                "{newer} > {older}"
            );
        }
    }

    /// A leading `v` is decoration, and a missing component is zero — so the
    /// three spellings of one version are one version.
    #[test]
    fn compare_ignores_a_leading_v_and_missing_components() {
        for (a, b) in [("1.2.0", "v1.2.0"), ("1.2", "1.2.0"), ("v26", "26.0.0")] {
            assert_eq!(compare(a, b), Ordering::Equal, "{a} == {b}");
        }
    }

    /// `compare` counts numbers and nothing else, so a pre-release compares
    /// *equal* to its release. The pair below is the reason: ranking it lower
    /// would also rank `26.8-dev` below `26.8`, and every development build
    /// would stop satisfying its own `min_saule_version`.
    #[test]
    fn compare_is_numeric_so_a_prerelease_equals_its_release() {
        assert_eq!(compare("v1.0.0-rc1", "v1.0.0"), Ordering::Equal);
        assert_eq!(compare("26.8-dev", "26.8"), Ordering::Equal);
        // Across versions the numbers decide, which is why passing over a
        // candidate is `is_prerelease`'s job and not this one's.
        assert_eq!(compare("v3.0.0-rc1", "v2.0.0"), Ordering::Greater);
    }

    #[test]
    fn a_prerelease_is_recognised_by_its_suffix() {
        for v in ["v1.0.0-rc1", "1.0.0-beta.2", "v2.0.0-alpha"] {
            assert!(is_prerelease(v), "{v}");
        }
        // `+build` is metadata that semver excludes from precedence.
        for v in ["v1.0.0", "1.0.0", "26.7", "v1.0.0+build7"] {
            assert!(!is_prerelease(v), "{v}");
        }
    }

    /// `version_at_least` is the `>=` question `min_saule_version` and
    /// `Saule.atLeast` ask, and it now asks it *through* `compare` — so this
    /// pins the two together.
    #[test]
    fn at_least_is_compare_with_equality_allowed() {
        assert!(version_at_least("26.7", "26.7"));
        assert!(version_at_least("26.7", "26"));
        assert!(version_at_least("26.7", "26.0"));
        assert!(version_at_least("26.10", "26.9"));
        assert!(!version_at_least("26.7", "26.8"));
        assert!(!version_at_least("26.7", "27.1"));
        // A development build satisfies the release it is heading toward.
        assert!(version_at_least("26.8-dev", "26.8"));
    }

    #[test]
    fn the_baked_in_version_is_year_dot_build() {
        assert_eq!(VERSION, format!("{YEAR}.{BUILD}"));
    }

    #[test]
    fn full_only_decorates_development_builds() {
        if IS_DEV {
            assert!(FULL.starts_with(&format!("{VERSION}-dev")), "{FULL}");
        } else {
            assert_eq!(FULL, VERSION);
        }
    }

    #[test]
    fn equal_versions_satisfy_each_other() {
        assert!(version_at_least("26.7", "26.7"));
    }

    #[test]
    fn a_higher_build_satisfies_a_lower_requirement() {
        assert!(version_at_least("26.7", "26.6"));
        assert!(!version_at_least("26.6", "26.7"));
    }

    #[test]
    fn the_year_outranks_the_build_number() {
        // The whole reason build numbers can reset each year.
        assert!(version_at_least("27.1", "26.412"));
        assert!(!version_at_least("26.412", "27.1"));
    }

    #[test]
    fn missing_components_are_zero() {
        assert!(version_at_least("26.7", "26"));
        assert!(version_at_least("26.0", "26"));
        assert!(!version_at_least("26", "26.1"));
    }

    #[test]
    fn a_dev_build_satisfies_the_release_it_heads_toward() {
        // `26.8-dev` is the work that becomes 26.8, so code requiring 26.8
        // must be developable on it.
        assert!(version_at_least("26.8-dev+1a2b3c4", "26.8"));
        assert!(!version_at_least("26.8-dev", "26.9"));
    }

    #[test]
    fn a_leading_v_is_accepted_on_either_side() {
        assert!(version_at_least("v26.7", "26.7"));
        assert!(version_at_least("26.7", "v26.7"));
    }

    #[test]
    fn the_old_calendar_scheme_does_not_silently_pass() {
        // Pre-26.x configs said `2026.1.0`. Under numeric comparison that is
        // a *higher* year than 26, so it correctly fails rather than being
        // read as "some 2026 version" — which is why every such config had
        // to be migrated rather than left alone.
        assert!(!version_at_least("26.1", "2026.1.0"));
    }
}
