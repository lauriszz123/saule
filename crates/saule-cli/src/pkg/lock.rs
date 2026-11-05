//! `saule.lock`: the exact commit of every package this project uses.
//!
//! `dependencies:` says what a project wants, and a tag or a branch can move.
//! This file says what it got, so a fresh clone builds against the same code
//! and a package changing underneath a project is a decision rather than a
//! surprise. `saule install` restores what it records and `saule update` is
//! what moves it. It is meant to be committed.
//!
//! One line per package, sorted, so a change reads clearly in a diff:
//!
//! ```text
//! github.com/acme/uikit v1.2.0 0123456789abcdef0123456789abcdef01234567
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use saule_project::spec::PackageSpec;

pub(crate) const LOCK_FILE: &str = "saule.lock";

const HEADER: &str = "# Written by `saule install` and `saule update`. Commit this file.\n";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Locked {
    pub(crate) reference: String,
    pub(crate) commit: String,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Lock {
    entries: BTreeMap<String, Locked>,
}

fn path_in(root: &Path) -> PathBuf {
    root.join(LOCK_FILE)
}

/// What identifies a package whichever ref it is at.
fn key(spec: &PackageSpec) -> String {
    format!("{}/{}/{}", spec.host.domain(), spec.owner, spec.repo)
}

impl Lock {
    /// A missing or unreadable file is an empty lock. Nothing is pinned yet,
    /// and installing will write it.
    pub(crate) fn read(root: &Path) -> Lock {
        std::fs::read_to_string(path_in(root))
            .map(|text| Lock::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Lock {
        let mut entries = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split_whitespace();
            if let (Some(name), Some(reference), Some(commit)) =
                (fields.next(), fields.next(), fields.next())
            {
                entries.insert(
                    name.to_string(),
                    Locked {
                        reference: reference.to_string(),
                        commit: commit.to_string(),
                    },
                );
            }
        }
        Lock { entries }
    }

    fn render(&self) -> String {
        let mut out = HEADER.to_string();
        for (name, locked) in &self.entries {
            out.push_str(&format!("{name} {} {}\n", locked.reference, locked.commit));
        }
        out
    }

    /// Write the file, or remove it when nothing is pinned.
    pub(crate) fn write(&self, root: &Path) -> std::io::Result<()> {
        let path = path_in(root);
        if self.entries.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        std::fs::write(path, self.render())
    }

    pub(crate) fn get(&self, spec: &PackageSpec) -> Option<&Locked> {
        self.entries.get(&key(spec))
    }

    pub(crate) fn set(&mut self, spec: &PackageSpec, reference: String, commit: String) {
        self.entries.insert(key(spec), Locked { reference, commit });
    }

    /// Keep only the packages in `wanted`, so an entry goes when its
    /// dependency does.
    pub(crate) fn retain(&mut self, wanted: &[PackageSpec]) {
        let keep: Vec<String> = wanted.iter().map(key).collect();
        self.entries.retain(|name, _| keep.contains(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_round_trips_through_its_text() {
        let text = format!("{HEADER}github.com/a/b v1 abc\ngithub.com/c/d main def\n");
        let lock = Lock::parse(&text);
        assert_eq!(lock.entries.len(), 2);
        assert_eq!(lock.render(), text);
    }

    #[test]
    fn comments_and_malformed_lines_are_skipped() {
        let lock = Lock::parse("# note\n\ngithub.com/a/b v1\ngithub.com/c/d main def\n");
        assert_eq!(lock.entries.len(), 1);
    }
}
