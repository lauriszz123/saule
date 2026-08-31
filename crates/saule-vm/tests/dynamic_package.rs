//! Compiling an `import` of a dynamic native package.
//!
//! A dynamic package is a TOML manifest plus a shared library loaded with
//! `dlopen`. The manifest carries everything the *compiler* needs — class
//! names, method names, parameter names, arities — and is parsed without
//! touching the binary, so a package's exports fold into constants exactly
//! like a statically-linked one's. The `dlopen` is a runtime side effect and
//! stays one: it is recorded on the chunk and performed by `run_program`
//! immediately before the body of the module that imported it, which is
//! where the tree-walker resolves the same `import`.
//!
//! These tests need their own `SAULE_HOME`, and `discover()` runs once per
//! process, so this is a test *file* of its own rather than a module of the
//! differential suite.

use std::path::{Path, PathBuf};

/// A manifest naming a binary that is deliberately **not** installed.
///
/// That absence is the load-bearing part of every test here: compiling this
/// package's import has to succeed anyway, which it can only do if nothing
/// tried to open the library.
const MANIFEST: &str = r#"
[package]
name = "testpkg"
version = "0.1.0"
binary = "testpkg-not-installed.so, testpkg-not-installed.dll, testpkg-not-installed.dylib"

[exports.Graphics]
type = "class"
doc = "A package that is described but not installed."

  [[exports.Graphics.methods]]
  name = "circle"
  sig = "fn(mode: string, x: float, y: float, radius: float) -> nil"
  native_symbol = "testpkg_graphics_circle"
"#;

/// The `SAULE_HOME` these tests run under.
fn home() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dynpkg_home")
}

/// Point `SAULE_HOME` at a manifest directory holding [`MANIFEST`], then run
/// discovery. Idempotent, and every test calls it first.
fn install_manifest() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let home = home();
        let manifests = home.join("native_manifests");
        std::fs::create_dir_all(&manifests).expect("create manifest dir");
        std::fs::write(manifests.join("testpkg.toml"), MANIFEST).expect("write manifest");
        // No `native_packages/` directory at all — nothing to load, by design.

        // SAFETY: single-threaded, before any other thread in this test
        // binary has started and before the first `init()` reads it.
        unsafe { std::env::set_var("SAULE_HOME", &home) };
        saule_interpreter::init();
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
/// so the tests can tell *which* chunk records the load.
///
/// `name` per caller: `project` clears the directory it writes into, and
/// these tests run in parallel.
fn two_module_program(name: &str) -> PathBuf {
    project(
        name,
        &[
            (
                "lib.sau",
                "import * from testpkg\n\
                 export fn draw()\n\
                 \x20 Graphics.circle(\"fill\", 1.0, 2.0, 3.0)\n\
                 end\n",
            ),
            ("main.sau", "import draw from lib\ndraw()\n"),
        ],
    )
}

fn compile(entry: &Path) -> saule_vm::program::Program {
    match saule_vm::program::compile(entry) {
        Ok(p) => p,
        Err(e) => panic!("expected `{}` to compile: {e}", entry.display()),
    }
}

#[test]
fn an_import_of_a_dynamic_package_compiles() {
    install_manifest();
    // The gap this closes. Until the manifest became the compiler's source
    // of truth this was a deliberate `CompileError::Unsupported`, and every
    // program importing a package fell back to the tree-walker whole.
    let dir = two_module_program("dynpkg_compiles");
    let program = compile(&dir.join("main.sau"));
    assert_eq!(program.modules.len(), 2, "lib and main");
}

#[test]
fn compiling_records_the_load_without_performing_it() {
    install_manifest();
    let dir = two_module_program("dynpkg_records");
    let program = compile(&dir.join("main.sau"));

    // Post-order: an imported module precedes its importer.
    let lib = &program.modules[0];
    let main = program.entry_chunk();

    let packages: Vec<&str> = lib.dynamic_imports.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(packages, ["testpkg"], "the importing module records the load");
    assert!(
        main.dynamic_imports.is_empty(),
        "a module that imports no package records no load"
    );

    // And compiling got this far with no binary on disk — which is the whole
    // property. A compile that had opened the library could not have.
    assert!(
        !home().join("native_packages").exists(),
        "this fixture must not have an installed binary"
    );
}

#[test]
fn running_reports_the_missing_library_at_the_import() {
    install_manifest();
    let dir = two_module_program("dynpkg_runs");
    let program = compile(&dir.join("main.sau"));

    // Compiling deferred the load; running performs it, and here it fails.
    // The point is *that it is reported at all*, and as an import error —
    // folding a package's names at compile time must not let a program with
    // no library behind it run partway and fail at the first call instead.
    let err = saule_vm::run_program(program).expect_err("no library is installed");
    let text = err.to_string();
    assert!(
        text.contains("testpkg"),
        "the failure should name the package: {text}"
    );
}
