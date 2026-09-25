//! `SAULE_HOME`: where the toolchain keeps what it installs.
//!
//! ```text
//! ~/.saule/
//!   bin/                                  saule, saule-lsp
//!   native_packages/<lib>.{so,dylib,dll}  compiled packages, each self-describing
//!   packages/<host>/<owner>/<repo>/<ref>/ Saule source packages
//!   installed/<host>/<owner>/<repo>       what `saule install` put there
//!   tmp/                                  staging, so an install is a rename
//! ```
//!
//! Packages are installed **globally** and shared by every project; a project
//! only records *which* ones it uses, in `dependencies:`. Two projects wanting
//! different refs of one package get one directory each, which is why the ref
//! is part of the path.
//!
//! This lives here rather than next to the loader because both sides need it:
//! `saule-runtime` reads these directories to discover and resolve packages,
//! and `saule-cli` writes them.

use std::path::{Path, PathBuf};

use crate::spec::{Host, PackageSpec};

/// The Saule home directory — the root of everything the toolchain installs
/// per user.
///
/// `SAULE_HOME`, when set, **is** that directory — it is used verbatim, not
/// treated as a parent to append `.saule` to. This matches how the install
/// scripts interpret the variable. Unset, it defaults to `.saule` under the
/// user's home.
pub fn saule_home() -> PathBuf {
    if let Some(explicit) = std::env::var_os("SAULE_HOME") {
        return PathBuf::from(explicit);
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".saule")
}

/// Compiled packages: one library file each, carrying its own description.
pub fn native_packages_dir() -> PathBuf {
    saule_home().join("native_packages")
}

/// Saule source packages, keyed by host, owner, repo and ref.
pub fn source_packages_dir() -> PathBuf {
    saule_home().join("packages")
}

/// Where a source package's tree lives once installed.
pub fn source_package_path(spec: &PackageSpec, reference: &str) -> PathBuf {
    source_packages_dir()
        .join(spec.host.domain())
        .join(&spec.owner)
        .join(&spec.repo)
        .join(sanitise_ref(reference))
}

/// Staging for an install, so the final step is a rename within one
/// filesystem and an interrupted install leaves nothing half-written.
pub fn tmp_dir() -> PathBuf {
    saule_home().join("tmp")
}

/// Records of what `saule install` installed, one file per package.
pub fn receipts_dir() -> PathBuf {
    saule_home().join("installed")
}

/// The receipt for one package.
pub fn receipt_path(spec: &PackageSpec) -> PathBuf {
    receipts_dir()
        .join(spec.host.domain())
        .join(&spec.owner)
        .join(&spec.repo)
}

/// A ref made safe to use as one path component: `refs/heads/main` and a
/// tag with a slash in it are both legal git, and neither is a directory
/// name. Nothing else about the ref is changed, so the directory stays
/// readable.
fn sanitise_ref(reference: &str) -> String {
    reference.replace(['/', '\\'], "-")
}

/// Every host directory that could hold a receipt, for walking them all.
pub fn known_hosts() -> [Host; 1] {
    [Host::GitHub]
}

/// What one installed package is, as recorded at install time.
///
/// Written because it cannot be re-derived: whether a repository held a Saule
/// project or a Rust crate is a property of what was in it when it was
/// installed, and answering it again would mean another network fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// The ref actually installed — never empty, even when the entry in
    /// `dependencies:` omitted one, so what is on disk is always known.
    pub reference: String,
    /// The Saule import name: a source package's `name:`, or the name a
    /// native library declares.
    pub name: Option<String>,
    pub kind: ReceiptKind,
}

/// A repository is one of these. Never both: a package is a Saule project or
/// it is a native library, and which one decides how it installs and where
/// it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptKind {
    /// A compiled package: one file in `native_packages/`.
    Native { file: String },
    /// A Saule source tree under `packages/`.
    Source,
}

impl Receipt {
    /// Parse a receipt. Same `key: value` shape as `saule.config`, for the
    /// same reason: one format to read, and a file a person can inspect.
    pub fn parse(text: &str) -> Option<Receipt> {
        let (mut reference, mut kind, mut file, mut name) = (None, None, None, None);
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim().trim_matches('"').to_string();
            match key.trim() {
                "ref" => reference = Some(value),
                "kind" => kind = Some(value),
                "file" => file = Some(value),
                "name" => name = Some(value),
                _ => {}
            }
        }
        let kind = match kind.as_deref()? {
            "native" => ReceiptKind::Native { file: file? },
            "source" => ReceiptKind::Source,
            _ => return None,
        };
        Some(Receipt {
            reference: reference?,
            name,
            kind,
        })
    }

    pub fn render(&self) -> String {
        let mut out = String::from("-- Written by `saule install`.\n");
        out.push_str(&format!("ref: \"{}\"\n", self.reference));
        match &self.kind {
            ReceiptKind::Native { file } => {
                out.push_str("kind: \"native\"\n");
                out.push_str(&format!("file: \"{file}\"\n"));
            }
            ReceiptKind::Source => out.push_str("kind: \"source\"\n"),
        }
        if let Some(name) = &self.name {
            out.push_str(&format!("name: \"{name}\"\n"));
        }
        out
    }

    /// A short word for what this package is, for `saule list`.
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            ReceiptKind::Native { .. } => "native",
            ReceiptKind::Source => "source",
        }
    }

    /// Read the receipt for `spec`, if it is installed.
    pub fn read(spec: &PackageSpec) -> Option<Receipt> {
        let text = std::fs::read_to_string(receipt_path(spec)).ok()?;
        Receipt::parse(&text)
    }

    /// Write `self` as `spec`'s receipt, creating the directories it needs.
    pub fn write(&self, spec: &PackageSpec) -> std::io::Result<()> {
        let path = receipt_path(spec);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.render())
    }
}

/// Remove a directory that may not exist, treating "already gone" as done.
pub fn remove_dir_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Remove a file that may not exist, treating "already gone" as done.
pub fn remove_file_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Dependency;

    fn spec(raw: &str) -> PackageSpec {
        match Dependency::parse(raw).unwrap() {
            Dependency::Package(p) => p,
            Dependency::Path(_) => panic!("not a package"),
        }
    }

    #[test]
    fn a_source_package_is_keyed_by_host_owner_repo_and_ref() {
        let p = spec("gh:lauriszz123/uikit@v1.2.0");
        let path = source_package_path(&p, "v1.2.0");
        assert!(
            path.ends_with("github.com/lauriszz123/uikit/v1.2.0"),
            "{path:?}"
        );
    }

    /// A branch ref contains a slash on plenty of repositories, and a path
    /// component cannot.
    #[test]
    fn a_ref_with_a_slash_stays_one_directory() {
        let p = spec("gh:owner/repo");
        let path = source_package_path(&p, "refs/heads/main");
        assert!(path.ends_with("refs-heads-main"), "{path:?}");
    }

    #[test]
    fn a_receipt_round_trips() {
        for receipt in [
            Receipt {
                reference: "v0.1.0".into(),
                name: Some("shine".into()),
                kind: ReceiptKind::Native {
                    file: "libsaule_shine.dylib".into(),
                },
            },
            Receipt {
                reference: "main".into(),
                name: Some("uikit".into()),
                kind: ReceiptKind::Source,
            },
        ] {
            let parsed = Receipt::parse(&receipt.render()).expect("parses");
            assert_eq!(parsed, receipt);
        }
    }

    #[test]
    fn a_receipt_missing_its_kind_is_not_a_receipt() {
        assert!(Receipt::parse("ref: \"v1\"\n").is_none());
    }
}
