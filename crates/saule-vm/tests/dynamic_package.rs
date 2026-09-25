//! Compiling an `import` of a dynamic native package.
//!
//! A dynamic package is a shared library whose description — class names,
//! method names, parameter names, arities — is compiled into it as data and
//! read out of the file without loading it. That is everything the
//! *compiler* needs, so a package's exports fold into constants exactly like
//! a statically-linked one's. The `dlopen` is a runtime side effect and stays
//! one: it is recorded on the chunk and performed by `run_program`
//! immediately before the body of the module that imported it.
//!
//! These tests need their own `SAULE_HOME`, and `discover()` runs once per
//! process, so this is a test *file* of its own rather than a module of the
//! differential suite.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf()
}

/// Build `saule-native-fixture` and install its library as the only file in
/// a fresh `SAULE_HOME`, then run discovery. Idempotent, and every test
/// calls it first.
fn install_fixture() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // A target directory of its own: this runs while `cargo test` may
        // still hold the workspace's.
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-fixture-target");
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "--quiet",
                "-p",
                "saule-native-fixture",
                "--target-dir",
            ])
            .arg(&target)
            .current_dir(workspace_root())
            .status()
            .expect("run cargo to build the fixture package");
        assert!(status.success(), "building saule-native-fixture failed");
        let file = format!(
            "{}saule_fixture{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        );

        let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dynpkg_home");
        let packages = home.join("native_packages");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&packages).expect("create the packages directory");
        std::fs::copy(target.join("debug").join(&file), packages.join(&file))
            .expect("install the fixture library");

        // SAFETY: single-threaded, before any other thread in this test
        // binary has started and before the first `init()` reads it.
        unsafe { std::env::set_var("SAULE_HOME", &home) };
        saule_runtime::init();
    });
}

/// Write a throwaway project and hand back its directory. Mirrors the helper
/// in `tests/program.rs`; files land under `target/` so a failing test leaves
/// them behind for inspection.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create project dir");
    for (file, body) in files {
        std::fs::write(dir.join(file), body).expect("write module");
    }
    dir
}

/// A two-module program: the package is imported by `lib`, not by the entry,
/// so the test can tell *which* chunk records the load.
fn two_module_program(name: &str) -> PathBuf {
    project(
        name,
        &[
            (
                "lib.sau",
                "import * from fixture\n\
                 export fn total() -> integer\n\
                 \x20 local c = Counter(40)\n\
                 \x20 c.bump()\n\
                 \x20 return c.value + Util.sum({1})\n\
                 end\n",
            ),
            ("main.sau", "import total from lib\nprintln(total())\n"),
        ],
    )
}

fn compile(entry: &Path) -> saule_vm::program::Program {
    match saule_vm::program::compile(entry) {
        Ok(p) => p,
        Err(e) => panic!("expected `{}` to compile: {e:?}", entry.display()),
    }
}

/// Compiling folds the package's exports without loading it, records the
/// load on the importing module's chunk, and running performs it.
///
/// One test, in order, because the loaded-library cache is process-wide: a
/// test running a program in parallel would load the library and make
/// "compiling loaded nothing" unobservable.
#[test]
fn compiling_records_the_load_and_running_performs_it() {
    install_fixture();
    let dir = two_module_program("dynpkg_compile_then_run");
    let program = compile(&dir.join("main.sau"));

    // Post-order: an imported module precedes its importer.
    assert_eq!(program.modules.len(), 2, "lib and main");
    let lib = &program.modules[0];
    let main = program.entry_chunk();
    let packages: Vec<&str> = lib
        .dynamic_imports
        .iter()
        .map(|(p, _)| p.as_str())
        .collect();
    assert_eq!(
        packages,
        ["fixture"],
        "the importing module records the load"
    );
    assert!(
        main.dynamic_imports.is_empty(),
        "a module that imports no package records no load"
    );

    // The whole property: the program compiled — constructor, method,
    // property and static call included — and no library was opened.
    assert!(
        !saule_runtime::dynamic_packages::is_loaded("fixture"),
        "compiling must not load a package"
    );

    let (printed, ran) = saule_runtime::output::capture(|| saule_vm::run_program(program));
    ran.expect("the program runs");
    assert_eq!(printed.text(), "42\n");
    assert!(
        saule_runtime::dynamic_packages::is_loaded("fixture"),
        "running performs the load"
    );
}
