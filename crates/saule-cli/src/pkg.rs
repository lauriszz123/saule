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
    let spec = parse_install_target(raw).unwrap_or_else(|e| fail(e));

    let installed = match install_package(&spec) {
        Ok(i) => i,
        Err(e) => fail(e),
    };

    // Record it, so a fresh clone of this project can reinstall it. Outside
    // a project there is nothing to record — the package is still installed
    // and usable, and saying so is better than refusing.
    let entry = PackageSpec {
        reference: Some(installed.receipt.reference.clone()),
        ..spec.clone()
    }
    .to_entry();

    match project_root() {
        Some(root) => {
            let path = config_path(&root);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| fail(format!("reading {}: {e}", path.display())));
            let edited = edit::add_dependency(&text, &entry);
            if edited.changed {
                std::fs::write(&path, &edited.text)
                    .unwrap_or_else(|e| fail(format!("writing {}: {e}", path.display())));
                println!("  recorded  {entry} in {}", saule_project::CONFIG_FILE);
            }
            if !edited.changed && !text.contains(&entry) {
                // The one case the editor declines: a list written across
                // several lines. Say so rather than silently not recording.
                println!(
                    "  note      could not update `dependencies:` automatically — \
                     add {entry} to it by hand"
                );
            }
        }
        None => println!("  note      not inside a project, so nothing was recorded"),
    }

    println!("\n{}", usage_hint(&installed));
}

/// Read what was typed at `saule install` as a package.
///
/// The `gh:` prefix is optional here. A *config entry* needs it, because
/// `foo/bar` in `dependencies:` is genuinely ambiguous with a subdirectory —
/// but on the command line there is no such reading: nothing local is being
/// referenced, and `saule install acme/uikit` can only mean one thing.
fn parse_install_target(raw: &str) -> Result<PackageSpec, String> {
    match Dependency::parse(raw)? {
        Dependency::Package(p) => Ok(p),
        Dependency::Path(path) => {
            if let Ok(Dependency::Package(p)) = Dependency::parse(&format!("gh:{path}"))
                && looks_like_a_slug(&path)
            {
                return Ok(p);
            }
            Err(format!(
                "`{path}` is a path, not a package.\n  \
                 A path dependency is used where it lies — add it to `dependencies:` \
                 directly, there is nothing to install.\n  \
                 To install from GitHub, write `saule install <owner>/<repo>`."
            ))
        }
    }
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
    if let Some(commit) = fetch::short_commit(&fetched.dir) {
        println!("  at        {} ({commit})", fetched.reference);
    }

    let has_config = fetched.dir.join(saule_project::CONFIG_FILE).is_file();
    let has_cargo = fetched.dir.join("Cargo.toml").is_file();

    let receipt = if has_config {
        install_source(spec, fetched)?
    } else if has_cargo {
        install_native(fetched)?
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
fn install_source(spec: &PackageSpec, fetched: &fetch::Fetched) -> Result<Receipt, String> {
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
        name: Some(name),
        kind: ReceiptKind::Source,
    })
}

/// A Rust crate: build it, and install the library it produces.
fn install_native(fetched: &fetch::Fetched) -> Result<Receipt, String> {
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
        match found.or_else(|| parse_install_target(target).ok()) {
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
