//! `saule install`, `saule remove`, `saule list` — packages from GitHub.
//!
//! Packages are installed **globally**, into `SAULE_HOME`, and shared by
//! every project on the machine; a project records only *which* ones it uses,
//! as `gh:` entries in the `dependencies:` it already had. So a fresh clone of
//! a project is `saule install` away from building, and two projects wanting
//! the same package fetch it once.
//!
//! What a repository turns out to be decides how it is installed:
//!
//! * a **Saule project** (it has a `saule.config`) has its source installed
//!   under `packages/`, and its own `name:` is the import name — which is why
//!   Saule needs no package index;
//! * a **Rust crate** (a `Cargo.toml`, no `saule.config`) is a native package:
//!   it is built with `cargo build --release` and the library it produces goes
//!   into `native_packages/`, carrying its own description.
//!
//! Relative paths in `dependencies:` are untouched by all of this and keep
//! meaning exactly what they always have — see `saule_project::spec`.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::process;

use saule_project::home::{self, Receipt, ReceiptKind};
use saule_project::spec::{Dependency, PackageSpec};

mod edit;
mod fetch;
#[cfg(target_os = "macos")]
mod macho;
mod native;

/// Exit after printing `message` as an error, the way every other command
/// in this binary reports one.
fn fail(message: impl AsRef<str>) -> ! {
    eprintln!("error: {}", message.as_ref());
    process::exit(1)
}

/// The project whose `saule.config` records what is installed, if the
/// current directory is inside one.
fn project_root() -> Option<PathBuf> {
    saule_project::find_root(Path::new("."))
}

fn config_path(root: &Path) -> PathBuf {
    root.join(saule_project::CONFIG_FILE)
}

// ─── install ────────────────────────────────────────────────────────────────

/// `saule install [<package>]`.
///
/// With a package, fetch and install it and record it in this project. With
/// nothing, install everything this project records — the fresh-clone case.
pub(crate) fn cmd_install(target: Option<String>) {
    match target {
        Some(raw) => install_one(&raw),
        None => install_all(),
    }
}

fn install_one(raw: &str) {
    match install_target(raw).unwrap_or_else(|e| fail(e)) {
        Target::Path(dir) => install_path(&dir),
        Target::Package(spec) => install_from_host(&spec),
    }
}

/// Install a package from its host, and record it in this project.
fn install_from_host(spec: &PackageSpec) {
    let installed = match install_package(spec) {
        Ok(i) => i,
        Err(e) => fail(e),
    };

    // Record it, so a fresh clone of this project can reinstall it. Outside a
    // project there is nothing to record — the package is still installed and
    // usable, and saying so is better than refusing.
    match project_root() {
        Some(root) => record(&root, spec, &installed),
        None => println!("  note      not inside a project, so nothing was recorded"),
    }

    println!("\n{}", usage_hint(&installed));
}

/// What `saule install <target>` was pointed at.
enum Target {
    /// A package to fetch from a host.
    Package(PackageSpec),
    /// A directory on this machine, already canonicalised.
    Path(PathBuf),
}

/// Read what was typed at `saule install`.
///
/// The `gh:` prefix and the `owner@repo` `@` are both optional here. A *config
/// entry* needs one, because `foo/bar` in `dependencies:` is genuinely
/// ambiguous with a subdirectory — but on the command line the filesystem
/// settles it: a directory that is actually there is a path, and anything else
/// shaped like `owner/repo` can only mean a package.
fn install_target(raw: &str) -> Result<Target, String> {
    match Dependency::parse(raw)? {
        Dependency::Package(p) => Ok(Target::Package(p)),
        Dependency::Path(path) => {
            // A directory that exists is a path, whatever it is shaped like.
            // This is what lets `saule install ../uikit` mean the sibling
            // directory while `saule install acme/uikit` still reaches GitHub.
            let expanded = saule_project::expand_tilde(&path);
            if expanded.exists() {
                let canonical = expanded
                    .canonicalize()
                    .map_err(|e| format!("`{path}`: {e}"))?;
                return Ok(Target::Path(canonical));
            }
            // Nothing local by that name, so `owner/repo` can only be a
            // package. Kept because it is the spelling GitHub itself shows.
            if looks_like_a_slug(&path)
                && let Ok(Dependency::Package(p)) = Dependency::parse(&format!("gh:{path}"))
            {
                return Ok(Target::Package(p));
            }
            Err(format!(
                "`{path}` is neither a directory that exists nor a package.\n  \
                 To install from GitHub, write `saule install <owner>@<repo>`.\n  \
                 To use a project on this machine, give a path that exists."
            ))
        }
    }
}

/// `saule install <path>` — a project on this machine.
///
/// Nothing is copied and nothing is fetched: a path dependency is used
/// **where it lies**, so edits to it are visible to this project immediately,
/// which is what makes this the right shape for a library you are developing
/// alongside its consumer. Recording the path is therefore the whole install.
///
/// Written into `dependencies:` relative to the project root wherever one
/// path can reach the other, so the entry keeps working for anyone who clones
/// the pair together — and absolute otherwise, which is the best a different
/// drive allows.
fn install_path(dir: &Path) {
    let config = dir.join(saule_project::CONFIG_FILE);
    if !config.is_file() {
        fail(format!(
            "`{}` is not a Saule project — it has no `{}`.\n  \
             `saule init <name> --lib` creates one.",
            saule_project::user_path(dir),
            saule_project::CONFIG_FILE
        ));
    }
    let name = saule_project::Config::read(&config)
        .unwrap_or_else(|e| fail(format!("reading {}: {e}", config.display())))
        .name_or_dir(dir);

    let Some(root) = project_root() else {
        fail(format!(
            "not inside a Saule project — no `{}` here or above.\n  \
             A path dependency is recorded in the project that uses it, so there \
             has to be one.",
            saule_project::CONFIG_FILE
        ))
    };

    // `find_root` returns the path it walked up from the cwd, while `dir` came
    // back from `canonicalize` — on Windows that is a `\\?\` verbatim path, which
    // shares not one component with the ordinary form. Both sides have to be
    // canonical or the comparison below quietly fails and every entry is
    // written as an absolute path.
    let root = root.canonicalize().unwrap_or(root);

    if root == dir {
        fail("that is this project — a project cannot depend on itself.");
    }

    let entry = relative_to(&root, dir).unwrap_or_else(|| dir.to_string_lossy().into_owned());
    println!("  linked    {name} -> {}", saule_project::user_path(dir));

    let path = config_path(&root);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| fail(format!("reading {}: {e}", path.display())));
    let edited = edit::add_dependency(&text, &entry);
    if edited.changed {
        std::fs::write(&path, &edited.text)
            .unwrap_or_else(|e| fail(format!("writing {}: {e}", path.display())));
        println!("  recorded  {entry} in {}", saule_project::CONFIG_FILE);
    } else if text.contains(&entry) {
        println!("  present   {entry} is already a dependency");
    } else {
        println!(
            "  note      could not update `dependencies:` automatically — \
             add {entry} to it by hand"
        );
    }

    println!("\n    import * from \"{name}\"");
    println!(
        "\nUsed where it lies, so your edits to it take effect immediately — \
         nothing was copied."
    );
}

/// `target` written relative to `root`, with `/` separators.
///
/// `None` when the two share no prefix at all, which on Windows means
/// separate drives and so no relative path exists. Deliberately not
/// `canonicalize`-dependent: both sides are already canonical here, which is
/// what makes a plain component comparison correct.
fn relative_to(root: &Path, target: &Path) -> Option<String> {
    let root: Vec<_> = root.components().collect();
    let target: Vec<_> = target.components().collect();
    let shared = root
        .iter()
        .zip(target.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if shared == 0 {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();
    for _ in shared..root.len() {
        parts.push("..".to_string());
    }
    for c in &target[shared..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    // A path that goes nowhere is the project itself, which the caller has
    // already refused; `.` is the honest answer rather than an empty string.
    if parts.is_empty() {
        return Some(".".to_string());
    }
    // A sibling reads better as `../uikit` than `..\uikit`, and `/` works on
    // both platforms — the config is a file people read and share.
    Some(parts.join("/"))
}

/// Whether `raw` reads as `owner/repo` rather than as a path: one separator,
/// neither half empty, and none of the things that only a path has.
fn looks_like_a_slug(raw: &str) -> bool {
    let bare = raw.split('@').next().unwrap_or(raw);
    !bare.starts_with('.')
        && !bare.starts_with('/')
        && !bare.starts_with('~')
        && !bare.contains('\\')
        && bare.split('/').count() == 2
        && bare.split('/').all(|part| !part.is_empty())
}

fn install_all() {
    let Some(root) = project_root() else {
        fail(format!(
            "not inside a Saule project — no `{}` here or above.\n  \
             Name a package to install: `saule install gh:<owner>/<repo>`",
            saule_project::CONFIG_FILE
        ))
    };
    let config = saule_project::Config::read_in(&root)
        .unwrap_or_else(|e| fail(format!("reading {}: {e}", config_path(&root).display())));

    let packages: Vec<PackageSpec> = config
        .dependencies
        .iter()
        .filter_map(|raw| Dependency::parse(raw).ok())
        .filter_map(|d| d.as_package().cloned())
        .collect();

    if packages.is_empty() {
        println!(
            "Nothing to install: this project declares no packages (only paths, which \
             are used where they lie)."
        );
        return;
    }

    let mut installed = 0;
    let mut skipped = 0;
    for spec in &packages {
        // Already at the recorded ref: leave it. Reinstalling would refetch
        // and rebuild every package on every call.
        if let Some(receipt) = Receipt::read(spec)
            && spec
                .reference
                .as_ref()
                .is_none_or(|r| *r == receipt.reference)
            && is_present(spec, &receipt)
        {
            println!("  present   {} ({})", spec.slug(), receipt.reference);
            skipped += 1;
            continue;
        }
        match install_package(spec) {
            Ok(_) => installed += 1,
            Err(e) => fail(format!("installing `{}`: {e}", spec.slug())),
        }
    }
    println!("\n{installed} installed, {skipped} already present.");
}

/// What an install produced.
struct Installed {
    spec: PackageSpec,
    receipt: Receipt,
}

/// Fetch, build if it needs building, install globally, and write a receipt.
fn install_package(spec: &PackageSpec) -> Result<Installed, String> {
    // An entry with no `:version` means "the newest release", which is a
    // question only the remote can answer — so it is asked here rather than
    // left to mean "the default branch", which is what it used to mean.
    let resolved = resolve_version(spec)?;
    let spec = &resolved;
    println!(
        "  fetching  {} ({})",
        spec.slug(),
        spec.reference.as_deref().unwrap_or("default branch")
    );

    // Staged in `tmp/` and moved into place, so an interrupted install
    // leaves nothing half-written where a package is expected to be.
    let staging = home::tmp_dir().join(format!("install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let fetched = fetch::clone(spec, &staging)?;
    let result = install_fetched(spec, &fetched);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn install_fetched(spec: &PackageSpec, fetched: &fetch::Fetched) -> Result<Installed, String> {
    // Recorded in the receipt, not only printed: it is the only thing that
    // can tell a later `saule update` whether a package tracking a *branch*
    // has moved, since the ref name alone never changes.
    let commit = fetch::short_commit(&fetched.dir);
    if let Some(commit) = &commit {
        println!("  at        {} ({commit})", fetched.reference);
    }

    let has_config = fetched.dir.join(saule_project::CONFIG_FILE).is_file();
    let has_cargo = fetched.dir.join("Cargo.toml").is_file();

    let receipt = if has_config {
        install_source(spec, fetched, commit)?
    } else if has_cargo {
        install_native(fetched, commit)?
    } else {
        return Err(format!(
            "`{}` is not a Saule package: it has neither a `{}` nor a `Cargo.toml`",
            spec.slug(),
            saule_project::CONFIG_FILE
        ));
    };

    receipt
        .write(spec)
        .map_err(|e| format!("recording the install: {e}"))?;
    Ok(Installed {
        spec: spec.clone(),
        receipt,
    })
}

/// A Saule source package: its tree goes under `packages/`, keyed by ref.
fn install_source(
    spec: &PackageSpec,
    fetched: &fetch::Fetched,
    commit: Option<String>,
) -> Result<Receipt, String> {
    let config = saule_project::Config::read_in(&fetched.dir)
        .map_err(|e| format!("reading the package's config: {e}"))?;
    let name = config.name_or_dir(&fetched.dir);

    let dest = home::source_package_path(spec, &fetched.reference);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    home::remove_dir_if_present(&dest).map_err(|e| format!("replacing {}: {e}", dest.display()))?;

    // A clone carries its own `.git`; the installed copy is a source tree,
    // not a checkout, and nothing reads history out of it.
    let _ = std::fs::remove_dir_all(fetched.dir.join(".git"));
    std::fs::rename(&fetched.dir, &dest)
        .map_err(|e| format!("installing into {}: {e}", dest.display()))?;

    println!(
        "  installed {} {} -> {}",
        name,
        fetched.reference,
        dest.display()
    );
    Ok(Receipt {
        reference: fetched.reference.clone(),
        commit,
        name: Some(name),
        kind: ReceiptKind::Source,
    })
}

/// A Rust crate: build it, and install the library it produces.
fn install_native(fetched: &fetch::Fetched, commit: Option<String>) -> Result<Receipt, String> {
    println!("  building  cargo build --release");
    let built = native::build(&fetched.dir)?;
    let file = native::install(&built)?;
    let dir = home::native_packages_dir();
    println!("  installed {file} -> {}", dir.display());

    // The package's Saule name is compiled into the library, so the loader
    // already knows it; reading it back is only so `saule list` can show what
    // an `import` should say.
    let name = installed_package_name(&dir.join(&file));
    Ok(Receipt {
        reference: fetched.reference.clone(),
        commit,
        name,
        kind: ReceiptKind::Native { file },
    })
}

/// The Saule import name a built library declares, read back out of it.
fn installed_package_name(path: &Path) -> Option<String> {
    saule_runtime::dynamic_packages::discover();
    saule_runtime::dynamic_packages::package_names()
        .into_iter()
        .find(|name| {
            saule_runtime::dynamic_packages::package_path(name).is_some_and(|p| same_file(&p, path))
        })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Whether what a receipt describes is still on disk.
fn is_present(spec: &PackageSpec, receipt: &Receipt) -> bool {
    match &receipt.kind {
        ReceiptKind::Native { file } => home::native_packages_dir().join(file).is_file(),
        ReceiptKind::Source => home::source_package_path(spec, &receipt.reference).is_dir(),
    }
}

/// The line to type next, which is the thing a person actually wants after
/// an install finishes.
fn usage_hint(installed: &Installed) -> String {
    match &installed.receipt.name {
        Some(name) => format!("    import * from \"{name}\""),
        None => format!(
            "Installed. See `{}`'s documentation for what it exports.",
            installed.spec.slug()
        ),
    }
}

/// Fill in a spec's absent version with the repository's newest tag.
///
/// `owner@repo` with no `:version` means the newest release. Resolving it
/// here rather than at the fetch is what lets the ref actually installed be
/// recorded in `dependencies:` as something specific — so a fresh clone of
/// the project gets the same version, and `saule update` has a "before" to
/// compare against.
///
/// A repository with no tags at all falls back to its default branch, which
/// is not an error: plenty of packages are developed on a branch and have
/// never cut a release.
fn resolve_version(spec: &PackageSpec) -> Result<PackageSpec, String> {
    if spec.reference.is_some() {
        return Ok(spec.clone());
    }
    match fetch::latest_tag(spec)? {
        Some(tag) => {
            println!("  newest    {} {tag}", spec.slug());
            Ok(PackageSpec {
                reference: Some(tag),
                ..spec.clone()
            })
        }
        None => {
            println!(
                "  note      {} has no tags — taking the default branch",
                spec.slug()
            );
            Ok(spec.clone())
        }
    }
}

// ─── update ─────────────────────────────────────────────────────────────────

/// Where one package stands against its repository.
enum Status {
    /// Nothing newer exists. Carries what is installed, for the report.
    Current(String),
    /// Something newer does. `reference` is what to install to get it, which
    /// is not always `to`: a package tracking a branch is reinstalled from
    /// that *same* branch to pick up a new commit.
    Behind {
        from: String,
        to: String,
        reference: String,
    },
    /// In `dependencies:` but not on disk. `saule install` is the fix, and
    /// saying so beats reporting it as up to date.
    Missing,
    /// Could not be asked: no network, or the repository is gone. Reported
    /// rather than swallowed, because "nothing newer" and "could not tell"
    /// are very different answers to give someone before a release.
    Unknown(String),
}

/// `saule update [<package>] [--check]`.
///
/// Updating means moving to the newest tag. There are no version ranges to
/// re-resolve — an entry always records an exact ref — so this is the only
/// thing that ever rewrites one.
pub(crate) fn cmd_update(target: Option<String>, check: bool) {
    let Some(root) = project_root() else {
        fail(format!(
            "not inside a Saule project — no `{}` here or above.",
            saule_project::CONFIG_FILE
        ))
    };
    let config = saule_project::Config::read_in(&root)
        .unwrap_or_else(|e| fail(format!("reading {}: {e}", config_path(&root).display())));

    let mut packages: Vec<PackageSpec> = config
        .dependencies
        .iter()
        .filter_map(|raw| Dependency::parse(raw).ok())
        .filter_map(|d| d.as_package().cloned())
        .collect();

    // Naming one package narrows the work to it, matched the way `saule
    // remove` matches — so whatever you know the package by works here too,
    // including the import name it was installed under.
    if let Some(needle) = &target {
        let one = find_package(&root, needle)
            .or_else(|| packages.iter().find(|s| s.matches(needle)).cloned())
            .unwrap_or_else(|| {
                fail(format!(
                    "`{needle}` is not a package this project depends on.\n  \
                     `saule list` shows what is."
                ))
            });
        packages.retain(|s| s.host == one.host && s.owner == one.owner && s.repo == one.repo);
    }

    if packages.is_empty() {
        println!(
            "Nothing to update: this project declares no packages (only paths, which \
             are used where they lie)."
        );
        return;
    }

    println!("{}", root.display());
    let statuses: Vec<(PackageSpec, Status)> = packages
        .into_iter()
        .map(|spec| {
            let status = status_of(&spec);
            report(&spec, &status);
            (spec, status)
        })
        .collect();

    let behind = statuses
        .iter()
        .filter(|(_, s)| matches!(s, Status::Behind { .. }))
        .count();
    let unknown = statuses
        .iter()
        .filter(|(_, s)| matches!(s, Status::Unknown(_)))
        .count();

    if check {
        println!();
        if behind == 0 {
            println!("Everything is up to date.");
        } else {
            let plural = if behind == 1 { "" } else { "s" };
            println!("{behind} package{plural} can be updated. Run `saule update` to do it.");
        }
        // Non-zero so this gates CI the way `saule check` does, and two
        // distinct codes because a script wants to tell "something is behind"
        // from "I could not find out".
        if unknown > 0 {
            process::exit(2);
        }
        if behind > 0 {
            process::exit(1);
        }
        return;
    }

    if behind == 0 {
        println!("\nEverything is up to date.");
        if unknown > 0 {
            process::exit(2);
        }
        return;
    }

    println!();
    let mut updated = 0;
    for (spec, status) in &statuses {
        let Status::Behind { reference, to, .. } = status else {
            continue;
        };
        let wanted = PackageSpec {
            reference: Some(reference.clone()),
            ..spec.clone()
        };
        match install_package(&wanted) {
            Ok(installed) => {
                record(&root, &wanted, &installed);
                updated += 1;
            }
            // One package failing is no reason to leave the rest behind; the
            // summary line says how many made it.
            Err(e) => eprintln!("error: updating {} to {to}: {e}", spec.slug()),
        }
    }

    println!("\nUpdated {updated} of {behind} package(s).");
    if updated < behind {
        process::exit(1);
    }
}

/// Ask a package's repository where it stands.
fn status_of(spec: &PackageSpec) -> Status {
    let Some(receipt) = Receipt::read(spec) else {
        return Status::Missing;
    };
    if !is_present(spec, &receipt) {
        return Status::Missing;
    }

    let latest = match fetch::latest_tag(spec) {
        Ok(t) => t,
        Err(e) => return Status::Unknown(e),
    };

    match latest {
        // A tagged repository: the newest tag is the answer whatever the
        // installed ref is. Moving off a branch onto a first release counts
        // as an update, which is why this does not require the installed ref
        // to be a tag.
        Some(tag) => {
            if saule_version::compare(&tag, &receipt.reference) == Ordering::Greater {
                Status::Behind {
                    from: receipt.reference.clone(),
                    to: tag.clone(),
                    reference: tag,
                }
            } else {
                Status::Current(receipt.reference)
            }
        }
        // No tags anywhere, so the only thing that can have moved is the
        // branch. Compare commits instead of names.
        None => match fetch::remote_commit(spec, &receipt.reference) {
            None => Status::Unknown(format!(
                "`{}` has no tags, and no ref `{}` to compare",
                spec.slug(),
                receipt.reference
            )),
            Some(remote) => match &receipt.commit {
                // The receipt predates commits being recorded. Nothing to
                // compare — and calling it current would mean this package
                // never updates again, so offer the update instead.
                None => Status::Behind {
                    from: format!("{} (commit unrecorded)", receipt.reference),
                    to: format!("{} ({})", receipt.reference, short(&remote)),
                    reference: receipt.reference.clone(),
                },
                // The receipt holds an abbreviated commit and `ls-remote`
                // gives the full one, so this is a prefix test, not equality.
                Some(have) if remote.starts_with(have.as_str()) => {
                    Status::Current(format!("{} ({have})", receipt.reference))
                }
                Some(have) => Status::Behind {
                    from: format!("{} ({have})", receipt.reference),
                    to: format!("{} ({})", receipt.reference, short(&remote)),
                    reference: receipt.reference.clone(),
                },
            },
        },
    }
}

/// A commit abbreviated the way git abbreviates one, for a report that sits
/// next to receipts written by `rev-parse --short`.
fn short(commit: &str) -> String {
    commit.chars().take(7).collect()
}

/// Print one package's status, in the column layout the other commands use.
fn report(spec: &PackageSpec, status: &Status) {
    match status {
        Status::Current(reference) => {
            println!("  {:<10} {} [{reference}]", "current", spec.slug())
        }
        Status::Behind { from, to, .. } => {
            println!("  {:<10} {} {from} -> {to}", "update", spec.slug())
        }
        Status::Missing => println!(
            "  {:<10} {} — not installed, run `saule install`",
            "missing",
            spec.slug()
        ),
        Status::Unknown(why) => println!("  {:<10} {} — {why}", "unknown", spec.slug()),
    }
}

/// Write `installed` into the project's `dependencies:`, replacing whatever
/// entry named this package before.
///
/// Shared by `install` and `update` so the two cannot spell an entry
/// differently — which they would, each formatting its own.
fn record(root: &Path, spec: &PackageSpec, installed: &Installed) {
    let entry = PackageSpec {
        reference: Some(installed.receipt.reference.clone()),
        ..spec.clone()
    }
    .to_entry();

    let path = config_path(root);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| fail(format!("reading {}: {e}", path.display())));
    let edited = edit::add_dependency(&text, &entry);
    if edited.changed {
        std::fs::write(&path, &edited.text)
            .unwrap_or_else(|e| fail(format!("writing {}: {e}", path.display())));
        println!("  recorded  {entry} in {}", saule_project::CONFIG_FILE);
    } else if !text.contains(&entry) {
        // The one case the editor declines: a list written across several
        // lines. Say so rather than silently not recording.
        println!(
            "  note      could not update `dependencies:` automatically — \
             add {entry} to it by hand"
        );
    }
}

// ─── remove ─────────────────────────────────────────────────────────────────

/// `saule remove <package> [--purge]`.
pub(crate) fn cmd_remove(target: &str, purge: bool) {
    let mut acted = false;
    let root = project_root();

    // What the user types is whatever they know the package by, and after an
    // install the thing they know is the import name — `saule install` ends
    // by telling them to write `import * from "shine"`. So `shine` has to
    // work here, even though nothing in `dependencies:` spells it.
    let found = root.as_ref().and_then(|r| find_package(r, target));
    let needle = found
        .as_ref()
        .map(|s| s.to_entry())
        .unwrap_or_else(|| target.to_string());

    if let Some(root) = &root {
        let path = config_path(root);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| fail(format!("reading {}: {e}", path.display())));
        let (edited, removed) = edit::remove_dependency(&text, &needle);
        if edited.changed {
            std::fs::write(&path, &edited.text)
                .unwrap_or_else(|e| fail(format!("writing {}: {e}", path.display())));
            for entry in &removed {
                println!("  removed   {entry} from {}", saule_project::CONFIG_FILE);
            }
            acted = true;
        }
    }

    if purge {
        // The project's own entry first, so a bare import name works; failing
        // that, whatever was typed, so `--purge` works outside a project too.
        // Only a package can be purged — a path dependency's directory is
        // the user's own and is never deleted by `saule remove`.
        let named = || match install_target(target) {
            Ok(Target::Package(p)) => Some(p),
            _ => None,
        };
        match found.or_else(named) {
            Some(spec) => {
                if purge_package(&spec) {
                    acted = true;
                }
            }
            None => fail(format!(
                "`--purge` deletes an installed package, and `{target}` does not name \
                 one.\n  Try `saule remove <owner>/<repo> --purge`, or \
                 `saule list --global` to see what is installed."
            )),
        }
    }

    if !acted {
        eprintln!("error: nothing matched `{target}`.");
        eprintln!("  `saule list` shows this project's dependencies.");
        eprintln!("  `saule list --global` shows everything installed.");
        process::exit(1);
    }

    if !purge {
        println!(
            "\nThe installed copy is kept — other projects may use it. \
             `saule remove {target} --purge` deletes it."
        );
    }
}

/// The package this project records that `needle` names — by entry,
/// `owner/repo`, bare repo name, or the *import name* it was installed
/// under.
///
/// The import name lives in the receipt rather than in the entry, so this is
/// the only way to get from `shine` back to `gh:lauriszz123/saule-shine`
/// without a network fetch — and `shine` is exactly what `saule install`
/// told the user to type.
fn find_package(root: &Path, needle: &str) -> Option<PackageSpec> {
    let config = saule_project::Config::read_in(root).ok()?;
    let specs: Vec<PackageSpec> = config
        .dependencies
        .iter()
        .filter_map(|raw| Dependency::parse(raw).ok()?.as_package().cloned())
        .collect();
    specs
        .iter()
        .find(|s| s.matches(needle))
        .or_else(|| {
            specs
                .iter()
                .find(|s| Receipt::read(s).and_then(|r| r.name).as_deref() == Some(needle))
        })
        .cloned()
}

/// Delete an installed package's bytes and its receipt. Answers whether
/// anything was there.
fn purge_package(spec: &PackageSpec) -> bool {
    let Some(receipt) = Receipt::read(spec) else {
        return false;
    };
    match &receipt.kind {
        ReceiptKind::Native { file } => {
            let path = home::native_packages_dir().join(file);
            if let Err(e) = home::remove_file_if_present(&path) {
                fail(format!("deleting {}: {e}", path.display()));
            }
            println!("  deleted   {}", path.display());
        }
        ReceiptKind::Source => {
            let path = home::source_package_path(spec, &receipt.reference);
            if let Err(e) = home::remove_dir_if_present(&path) {
                fail(format!("deleting {}: {e}", path.display()));
            }
            println!("  deleted   {}", path.display());
        }
    }
    let _ = home::remove_file_if_present(&home::receipt_path(spec));
    true
}

// ─── list ───────────────────────────────────────────────────────────────────

/// `saule list [--global]`.
pub(crate) fn cmd_list(global: bool) {
    if global {
        list_global();
        return;
    }
    let Some(root) = project_root() else {
        fail(format!(
            "not inside a Saule project — no `{}` here or above.\n  \
             `saule list --global` shows everything installed on this machine.",
            saule_project::CONFIG_FILE
        ))
    };
    let config = saule_project::Config::read_in(&root)
        .unwrap_or_else(|e| fail(format!("reading {}: {e}", config_path(&root).display())));

    println!("{}", root.display());
    if config.dependencies.is_empty() {
        println!("  (no dependencies)");
        return;
    }

    for raw in &config.dependencies {
        match Dependency::parse(raw) {
            Ok(Dependency::Path(path)) => println!("  {:<10} {path}", "path"),
            Ok(Dependency::Package(spec)) => println!("  {}", describe(&spec)),
            Err(e) => println!("  {:<10} {raw} — {e}", "invalid"),
        }
    }
}

/// One project dependency, with what is actually installed for it.
fn describe(spec: &PackageSpec) -> String {
    let entry = spec.to_entry();
    let Some(receipt) = Receipt::read(spec) else {
        return format!(
            "{:<10} {entry} — not installed, run `saule install`",
            "missing"
        );
    };
    let name = receipt
        .name
        .as_deref()
        .map(|n| format!(" (import \"{n}\")"))
        .unwrap_or_default();
    // Two different kinds of wrong, and they want different fixes: a ref that
    // moved on is reinstalled, files that vanished are reinstalled too, but
    // saying which happened saves the reader guessing.
    let note = if !is_present(spec, &receipt) {
        " — files are missing, run `saule install`"
    } else if spec
        .reference
        .as_ref()
        .is_some_and(|r| *r != receipt.reference)
    {
        " — a different ref is installed, run `saule install`"
    } else {
        ""
    };
    format!(
        "{:<10} {entry}{name} [{}]{note}",
        receipt.kind_label(),
        receipt.reference
    )
}

/// Everything installed on this machine, whichever project asked for it.
fn list_global() {
    println!("{}", home::saule_home().display());
    let mut any = false;

    for spec in installed_specs() {
        if let Some(receipt) = Receipt::read(&spec) {
            let name = receipt
                .name
                .as_deref()
                .map(|n| format!(" (import \"{n}\")"))
                .unwrap_or_default();
            println!(
                "  {:<10} {}@{}{name}",
                receipt.kind_label(),
                spec.slug(),
                receipt.reference
            );
            any = true;
        }
    }

    // Libraries dropped into `native_packages/` by hand are usable and have
    // no receipt, so listing only what `saule install` put there would be a
    // misleading answer to "what is installed".
    let managed: Vec<String> = installed_specs()
        .into_iter()
        .filter_map(|s| Receipt::read(&s))
        .filter_map(|r| match r.kind {
            ReceiptKind::Native { file } => Some(file),
            ReceiptKind::Source => None,
        })
        .collect();
    let dir = home::native_packages_dir();
    let ext = native::library_extension();
    let mut unmanaged: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(ext) && !managed.contains(f))
        .collect();
    unmanaged.sort();
    for file in unmanaged {
        println!("  {:<10} {file} — installed by hand", "native");
        any = true;
    }

    if !any {
        println!("  (nothing installed)");
    }
}

/// Every package with a receipt, found by walking the receipts directory.
fn installed_specs() -> Vec<PackageSpec> {
    let mut out = Vec::new();
    for host in home::known_hosts() {
        let root = home::receipts_dir().join(host.domain());
        for owner in read_dir_names(&root) {
            for repo in read_dir_names(&root.join(&owner)) {
                out.push(PackageSpec {
                    host,
                    owner: owner.clone(),
                    repo,
                    reference: None,
                });
            }
        }
    }
    out.sort_by_key(|s| s.slug());
    out
}

fn read_dir_names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}
