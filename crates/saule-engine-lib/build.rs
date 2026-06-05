//! Build script: emit the package manifest next to the compiled artifact.
//!
//! Saule discovers native packages by their `<package>.toml` manifest. The
//! canonical manifest lives in the crate root (`engine.toml`) so it sits with
//! the code that implements it; this script copies it into Cargo's artifact
//! directory (`target/<profile>/engine.toml`) on every build. Install scripts
//! then read it from there rather than a separately maintained path.

use std::path::PathBuf;
use std::{env, fs};

fn main() {
    const MANIFEST: &str = "engine.toml";
    println!("cargo:rerun-if-changed={MANIFEST}");

    let src = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join(MANIFEST);

    // OUT_DIR is `<target>/<profile>/build/<pkg>-<hash>/out`; walk up three
    // levels to reach the artifact directory `<target>/<profile>/`, where the
    // compiled `.so`/`.dll`/`.dylib` also lands. This holds for custom
    // `CARGO_TARGET_DIR` and `--target <triple>` builds alike, since OUT_DIR
    // is always relative to the resolved profile directory.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let artifact_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR should have an artifact-directory ancestor");

    let dest = artifact_dir.join(MANIFEST);
    fs::copy(&src, &dest).unwrap_or_else(|e| {
        panic!(
            "failed to copy manifest {} -> {}: {e}",
            src.display(),
            dest.display()
        )
    });
}
