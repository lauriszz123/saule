//! `gen-manifest` — emit the package's `<name>.toml` manifest from the
//! `#[saule_export]` declarations in `saule-engine-lib`.
//!
//! Output path resolution:
//! - the first CLI argument, if given; otherwise
//! - `engine.toml` next to this executable (i.e. the Cargo artifact dir,
//!   alongside the compiled `saule_engine_lib.{so,dll,dylib}`).

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    // Reference every `#[saule_export]` so the linker retains their manifest
    // registrations when this binary links the crate as an rlib.
    saule_engine_lib::anchor();

    let toml = match saule_sdk::manifest::render() {
        Ok(toml) => toml,
        Err(e) => {
            eprintln!("gen-manifest: {e}");
            return ExitCode::FAILURE;
        }
    };

    let out = match std::env::args_os().nth(1) {
        Some(arg) => PathBuf::from(arg),
        None => match default_output() {
            Some(p) => p,
            None => {
                eprintln!("gen-manifest: could not determine output path; pass one as an argument");
                return ExitCode::FAILURE;
            }
        },
    };

    if let Err(e) = std::fs::write(&out, toml) {
        eprintln!("gen-manifest: failed to write {}: {e}", out.display());
        return ExitCode::FAILURE;
    }

    eprintln!("gen-manifest: wrote {}", out.display());
    ExitCode::SUCCESS
}

/// `engine.toml` next to the running executable (the artifact directory).
fn default_output() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("engine.toml"))
}
