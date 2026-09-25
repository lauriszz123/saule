//! Building a Rust package into a Saule native package, and installing it.
//!
//! A repository with a `Cargo.toml` and no `saule.config` is a native
//! package: a `cdylib` that carries its own description. Installing it means
//! building it and putting the one file it produces into
//! `native_packages/`, where the loader finds it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The library a build produced.
pub(crate) struct Built {
    pub(crate) path: PathBuf,
}

/// `cargo build --release` in `dir`, returning the `cdylib` it produced.
///
/// Output is streamed rather than captured: a release build of a real
/// package takes minutes, and a progress bar that appears only at the end
/// is indistinguishable from a hang.
pub(crate) fn build(dir: &Path) -> Result<Built, String> {
    let status = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "`cargo` is not installed, and this package is Rust that has to be built.\n  \
                 Install Rust from https://rustup.rs, or ask the package's author to publish \
                 prebuilt libraries."
                    .into()
            }
            _ => format!("could not run `cargo`: {e}"),
        })?;
    if !status.success() {
        return Err("the package failed to build — see cargo's output above".to_string());
    }

    let release = dir.join("target").join("release");
    let mut found = library_files(&release);
    match found.len() {
        0 => Err(format!(
            "the build produced no loadable library in `{}`.\n  \
             A Saule native package is a `cdylib`: its Cargo.toml needs \
             `crate-type = [\"cdylib\"]`.",
            release.display()
        )),
        1 => Ok(Built {
            path: found.remove(0),
        }),
        _ => {
            // Several would each claim to be the package. Which one is the
            // author's to say, and nothing in the repository says it.
            let names: Vec<String> = found
                .iter()
                .map(|p| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            Err(format!(
                "the build produced {} libraries ({}), so it is not clear which is the \
                 package. A repository should build exactly one.",
                found.len(),
                names.join(", ")
            ))
        }
    }
}

/// Every file in `dir` that this platform could load. Not recursive: cargo
/// puts the artifact at the top of the profile directory, and `deps/` holds
/// hashed copies of the same thing.
fn library_files(dir: &Path) -> Vec<PathBuf> {
    let ext = library_extension();
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some(ext))
        .collect();
    out.sort();
    out
}

/// The extension a loadable library has on this platform.
pub(crate) fn library_extension() -> &'static str {
    if cfg!(windows) {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// Copy `built` into `native_packages/`, repairing it if the platform
/// needs it, and answer the file name it was installed under.
pub(crate) fn install(built: &Built) -> Result<String, String> {
    let dir = saule_project::home::native_packages_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let name = built
        .path
        .file_name()
        .ok_or("the built library has no file name")?
        .to_string_lossy()
        .into_owned();
    let dest = dir.join(&name);
    std::fs::copy(&built.path, &dest).map_err(|e| format!("installing {}: {e}", dest.display()))?;

    #[cfg(target_os = "macos")]
    {
        // Apple's linker can leave a release build's string pool misaligned,
        // and macOS then refuses to load it. Repairing here is what makes
        // `saule install` produce something that works rather than something
        // that needs a second, undiscoverable step.
        match super::macho::align_string_pool(&dest) {
            Ok(0) => {}
            Ok(n) => println!("  repaired  padded the string pool by {n} bytes (macOS linker bug)"),
            Err(e) => {
                let _ = std::fs::remove_file(&dest);
                return Err(format!(
                    "the built library could not be made loadable on macOS: {e}"
                ));
            }
        }
    }

    Ok(name)
}
