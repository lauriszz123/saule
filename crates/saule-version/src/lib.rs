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
    let components = |v: &str| -> Vec<u32> {
        v.trim()
            .strip_prefix('v')
            .unwrap_or(v.trim())
            .split('.')
            .map(|part| {
                part.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let have = components(current);
    let want = components(required);
    for i in 0..have.len().max(want.len()) {
        let a = have.get(i).copied().unwrap_or(0);
        let b = want.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    true
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
