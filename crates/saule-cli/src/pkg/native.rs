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
        let mut msg = "the package failed to build — see cargo's output above".to_string();
        // Almost always the package's own fault, but there is one failure that
        // never is, and it is the first thing a Windows user hits.
        if let Some(hint) = toolchain_hint() {
            msg.push_str("\n  ");
            msg.push_str(&hint);
        }
        return Err(msg);
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

/// A hint about the *toolchain* when a native build fails, or `None` when
/// nothing is obviously wrong with it.
///
/// Diagnosed from the toolchain rather than from cargo's output, which this
/// function never sees: the build streams straight to the terminal, because a
/// release build of a real package takes minutes and a progress bar that
/// appears only at the end is indistinguishable from a hang. Reading the
/// condition directly is also the sturdier of the two — it does not depend on
/// how cargo words an error this year.
///
/// Runs only after a failure, so it costs nothing on the path that works.
fn toolchain_hint() -> Option<String> {
    let out = Command::new("rustc").arg("-vV").output().ok()?;
    let vv = String::from_utf8_lossy(&out.stdout);
    let host = vv
        .lines()
        .find_map(|l| l.strip_prefix("host: "))?
        .trim()
        .to_string();
    gnu_toolchain_hint(&host, || on_path("dlltool"))
}

/// The hint itself, with the two facts it depends on passed in.
///
/// Split out so it can be tested: the interesting case is a Windows host,
/// which is not where the test suite runs.
fn gnu_toolchain_hint(host: &str, dlltool_present: impl FnOnce() -> bool) -> Option<String> {
    if !host.ends_with("-pc-windows-gnu") || dlltool_present() {
        return None;
    }
    Some(format!(
        "Your default Rust toolchain targets `{host}`, which links through \
         mingw's `dlltool` — and `dlltool` is not on PATH, so anything \
         depending on `windows-sys` cannot link.\n  \
         This is a toolchain problem, not a problem with the package. The \
         usual fix is to build with MSVC instead, which needs no mingw:\n    \
         rustup default stable-x86_64-pc-windows-msvc\n  \
         Saule's own releases are built for MSVC, so that also matches the \
         toolchain the interpreter you are running was built with."
    ))
}

/// Is `name` an executable somewhere on `PATH`?
///
/// `PATHEXT` is not consulted; `.exe` is appended on Windows and that is the
/// only suffix a mingw `dlltool` ever carries.
fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path)
        .any(|dir| dir.join(name).is_file() || dir.join(format!("{name}.exe")).is_file())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The case that prompted this: a Windows host on the GNU target with no
    /// mingw installed. `saule install` reported only "the package failed to
    /// build", and the cause was never the package.
    #[test]
    fn a_gnu_windows_host_without_dlltool_is_explained() {
        let hint = gnu_toolchain_hint("x86_64-pc-windows-gnu", || false)
            .expect("a gnu host with no dlltool should be explained");
        assert!(hint.contains("dlltool"), "{hint}");
        assert!(
            hint.contains("rustup default stable-x86_64-pc-windows-msvc"),
            "the hint must name the fix: {hint}"
        );
        assert!(
            hint.contains("not a problem with the package"),
            "the hint must say whose fault it is not: {hint}"
        );
    }

    /// With mingw installed, the GNU target links fine and the failure is the
    /// package's own. Saying otherwise would send the reader somewhere useless.
    #[test]
    fn a_gnu_host_with_dlltool_gets_no_hint() {
        assert!(gnu_toolchain_hint("x86_64-pc-windows-gnu", || true).is_none());
    }

    /// Every other host, including MSVC — which never wants `dlltool` even
    /// when one is absent.
    #[test]
    fn other_hosts_get_no_hint() {
        for host in [
            "x86_64-pc-windows-msvc",
            "x86_64-unknown-linux-gnu",
            "aarch64-apple-darwin",
            "x86_64-unknown-linux-musl",
        ] {
            assert!(
                gnu_toolchain_hint(host, || false).is_none(),
                "{host} should get no mingw hint"
            );
        }
    }

    /// `-pc-windows-gnu` has to be the *end* of the triple, so a target that
    /// merely contains it is not mistaken for one.
    #[test]
    fn the_triple_is_matched_at_the_end() {
        assert!(gnu_toolchain_hint("x86_64-pc-windows-gnullvm", || false).is_none());
    }

    #[test]
    fn on_path_finds_something_that_exists_and_not_something_that_does_not() {
        // `sh` on unix, `cmd` on Windows — one of the two is always there.
        assert!(
            on_path("sh") || on_path("cmd"),
            "PATH lookup found neither `sh` nor `cmd`"
        );
        assert!(!on_path("saule-definitely-not-a-real-binary-9f3a"));
    }
}
