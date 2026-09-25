//! Native packages end to end, through the `saule` binary a user runs.
//!
//! `saule-native-fixture` is built as a real shared library and installed
//! into a throwaway `SAULE_HOME` — one file in `native_packages/`, nothing
//! else — and Saule programs import it. So everything between a Rust
//! `#[saule_class]` and a Saule `c.bump()` is exercised as it ships: the
//! metadata compiled into the library, discovery reading it back out of the
//! file, the checker's view of it, loading, calling, objects' lifetimes, and
//! the errors a user sees when something is wrong.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf()
}

/// Build the fixture package once per test run and return its library.
fn fixture_library() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
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
        target.join("debug").join(file)
    })
}

/// A fresh `SAULE_HOME` for one test, with `libraries` installed into its
/// packages directory under the given file names.
fn home(test: &str, libraries: &[(&Path, &str)]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("native-homes")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    let packages = dir.join("native_packages");
    std::fs::create_dir_all(&packages).expect("create the packages directory");
    for (from, name) in libraries {
        std::fs::copy(from, packages.join(name)).expect("install a library");
    }
    dir
}

/// A home with only the fixture installed, under the name cargo gave it.
fn fixture_home(test: &str) -> PathBuf {
    let lib = fixture_library();
    let name = lib.file_name().and_then(|n| n.to_str()).expect("file name");
    home(test, &[(lib, name)])
}

/// Run `saule <cmd>` on `source`, written to a script in `home`.
fn saule(home: &Path, cmd: &str, source: &str) -> Output {
    let script = home.join("main.sau");
    std::fs::write(&script, source).expect("write the script");
    Command::new(env!("CARGO_BIN_EXE_saule"))
        .arg(cmd)
        .arg(&script)
        .env("SAULE_HOME", home)
        // With `RUST_BACKTRACE` set a package's panic report is printed on
        // purpose, for its author; the tests check what a user sees.
        .env_remove("RUST_BACKTRACE")
        .output()
        .expect("run saule")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Error output with miette's layout taken out: it wraps a long message
/// across lines prefixed `│`, and a test cares about the words, not where
/// the terminal width broke them.
fn stderr(o: &Output) -> String {
    let raw = String::from_utf8_lossy(&o.stderr);
    let mut out = String::new();
    for line in raw.lines() {
        let line = line.trim_start();
        let line = line
            .strip_prefix('│')
            .or_else(|| line.strip_prefix('×'))
            .unwrap_or(line)
            .trim();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

/// Run a program that must succeed, and return what it printed.
fn run_ok(home: &Path, source: &str) -> String {
    let out = saule(home, "run", source);
    assert!(
        out.status.success(),
        "the program should run.\nstdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );
    stdout(&out)
}

/// Run a program that must fail, and return its error output.
fn run_err(home: &Path, source: &str) -> String {
    let out = saule(home, "run", source);
    assert!(
        !out.status.success(),
        "the program should fail.\nstdout:\n{}",
        stdout(&out)
    );
    stderr(&out)
}

// ─── Objects ────────────────────────────────────────────────────────────────

#[test]
fn a_native_class_has_a_constructor_methods_and_properties() {
    let home = fixture_home("class_surface");
    let out = run_ok(
        &home,
        r#"import * from "fixture"
local c: Counter = Counter(10)
println(c.value)
println(c.bump())
c.step = 5
println(c.step, c.bump())
println(c.doubled())
println(type(c), c)
"#,
    );
    assert_eq!(out, "10\n11\n5\t16\n32\nCounter\t<instance of Counter>\n");
}

#[test]
fn objects_pass_in_by_reference_and_come_back_as_objects() {
    let home = fixture_home("objects_in_and_out");
    let out = run_ok(
        &home,
        r#"import * from "fixture"
local a = Counter(3)
local b: Counter = a.fork()
println(a.absorb(b), b.value)
local p: Counter = Counter.parse("42")
println(p.value)
println(Util.total({a, b, p}))
println(Util.valueOrZero(nil), Util.valueOrZero(a))
println(Util.apply(a, (k) => k.value * 10))
"#,
    );
    assert_eq!(out, "6\t3\n42\n51\n0\t6\n60\n");
}

/// The package's destructor runs when — and only when — the last Saule value
/// holding the object goes away.
#[test]
fn an_object_is_dropped_with_its_last_reference() {
    let home = fixture_home("object_lifetime");
    let out = run_ok(
        &home,
        r#"import * from "fixture"
println(Counter.live())
local a: Counter? = Counter(1)
local b: Counter? = a
println(Counter.live())
a = nil
println(Counter.live())
b = nil
println(Counter.live())
"#,
    );
    assert_eq!(out, "0\n1\n1\n0\n");
}

/// An object the package keeps and hands out is the same object each time.
#[test]
fn a_shared_object_keeps_its_identity() {
    let home = fixture_home("shared_identity");
    let out = run_ok(
        &home,
        r#"import * from "fixture"
local a = Util.shared()
local b = Util.shared()
println(a == b, a == Counter(100))
a.bump()
println(b.value)
"#,
    );
    assert_eq!(out, "true\tfalse\n101\n");
}

#[test]
fn a_reentrant_call_cannot_alias_a_mutable_borrow() {
    let home = fixture_home("reentrancy");
    let err = run_err(
        &home,
        r#"import * from "fixture"
local c = Counter(1)
println(c.whileBumping(fn()
  c.bump()
end))
"#,
    );
    assert!(
        err.contains("this Counter is in use by a call that has not returned yet"),
        "{err}"
    );
    assert!(
        !err.contains("type error: type error"),
        "doubled prefix: {err}"
    );
}

#[test]
fn a_read_only_property_cannot_be_assigned() {
    let home = fixture_home("read_only");
    let err = run_err(
        &home,
        "import * from \"fixture\"\nlocal c = Counter(1)\nc.value = 3\n",
    );
    assert!(err.contains("`Counter.value` is read-only"), "{err}");
}

// ─── Enums, collections, multiple returns ───────────────────────────────────

#[test]
fn enums_cross_the_boundary_as_themselves() {
    let home = fixture_home("enums");
    let out = run_ok(
        &home,
        r#"import * from "fixture"
println(Util.round(2.1, Rounding.Up), Util.round(2.9, Rounding.Down))
local m: Rounding? = Util.preferred(true)
println(m == Rounding.Nearest, Util.preferred(false) == nil)
local ms = Util.modes()
println(ms[1] == Rounding.Down, ms[3] == Rounding.Nearest)
"#,
    );
    assert_eq!(out, "3\t2\ntrue\ttrue\ntrue\ttrue\n");
}

#[test]
fn collections_are_copied_and_tuples_spread() {
    let home = fixture_home("collections");
    let out = run_ok(
        &home,
        r#"import * from "fixture"
println(Util.sum({1, 2, 3, 4}))
local t = Util.tally({"a", "b", "a"})
println(t["a"], t["b"])
local q, r = Util.divmod(17, 5)
println(q, r)
println(Util.greet("ada"), Util.greet("bob", "!"))
"#,
    );
    assert_eq!(out, "10\n2\t1\n3\t2\nhello, ada.\thello, bob!\n");
}

// ─── Errors from the package ────────────────────────────────────────────────

#[test]
fn a_rust_error_is_a_saule_runtime_error() {
    let home = fixture_home("result_err");
    let err = run_err(&home, "import * from \"fixture\"\nUtil.divmod(1, 0)\n");
    assert!(err.contains("division by zero"), "{err}");
}

/// A panic in package code is reported at the call, with where it happened,
/// and neither aborts the interpreter nor spills Rust's own panic report.
#[test]
fn a_panic_is_a_saule_runtime_error() {
    let home = fixture_home("panic");
    let err = run_err(&home, "import * from \"fixture\"\nprintln(Util.boom())\n");
    assert!(err.contains("Util.boom panicked at"), "{err}");
    assert!(err.contains("kaboom"), "{err}");
    assert!(
        !err.contains("thread '"),
        "Rust's panic report leaked: {err}"
    );
}

#[test]
fn an_integer_that_does_not_fit_is_refused() {
    let home = fixture_home("narrow");
    let err = run_err(
        &home,
        "import * from \"fixture\"\nprintln(Util.narrow(5000000000))\n",
    );
    assert!(
        err.contains("argument `x` is 5000000000, which does not fit a i32"),
        "{err}"
    );
}

// ─── The checker's view ─────────────────────────────────────────────────────

/// `saule check` knows a native class as well as a Saule one, from the
/// metadata alone.
#[test]
fn the_checker_types_native_classes_and_enums() {
    let home = fixture_home("checker");
    let cases = [
        (
            "local c = Counter(1)\nc.bump(\"x\")",
            "`Counter.bump` expects 0 argument(s), got 1",
        ),
        (
            "local c = Counter(1)\nc.nope()",
            "no member `nope` on `Counter`",
        ),
        ("println(Counter.bump())", "no member `bump` on `Counter`"),
        (
            "local c = Counter(\"ten\")",
            "argument 1 of `Counter.init` expects `integer`, got `string`",
        ),
        (
            "local r: Rounding = Util.round(1.5, Rounding.Up)",
            "cannot assign value of type `integer` to variable of type `Rounding`",
        ),
        (
            "local n: integer = Util.round(1.5, \"Up\")",
            "argument 2 of `Util.round` expects `Rounding`, got `string`",
        ),
        (
            "local c = Counter(1)\nlocal s: string = c.value",
            "cannot assign value of type `integer` to variable of type `string`",
        ),
        (
            "local m = Rounding.Sideways",
            "enum `Rounding` has no variant `Sideways`",
        ),
    ];
    for (snippet, expected) in cases {
        let out = saule(
            &home,
            "check",
            &format!("import * from \"fixture\"\n{snippet}\n"),
        );
        let err = stderr(&out);
        assert!(!out.status.success(), "`{snippet}` should not check");
        assert!(
            err.contains(expected),
            "`{snippet}`: expected `{expected}` in:\n{err}"
        );
    }

    // And the well-typed program checks clean.
    let out = saule(
        &home,
        "check",
        "import * from \"fixture\"\nlocal c: Counter = Counter(1)\nlocal n: integer = c.bump()\n",
    );
    assert!(out.status.success(), "{}", stderr(&out));
}

// ─── Import errors ──────────────────────────────────────────────────────────

/// A missing module is reported at its `import`, not as the unknown types it
/// leaves behind further down.
#[test]
fn a_missing_module_is_reported_at_the_import() {
    let home = home("missing", &[]);
    for cmd in ["run", "check"] {
        let out = saule(
            &home,
            cmd,
            "import Timer from \"nosuchthing\"\nlocal t: float = Timer.getTime()\n",
        );
        let err = stderr(&out);
        assert!(
            err.contains("could not find module `nosuchthing`"),
            "{cmd}: {err}"
        );
        assert!(
            err.contains("import Timer from"),
            "{cmd}: not at the import: {err}"
        );
        assert!(
            !err.contains("type unknown"),
            "{cmd}: the knock-on error won: {err}"
        );
    }
}

/// A file in the packages directory that is not a library at all is
/// reported by the import that names it, with the reason.
#[test]
fn a_library_that_is_not_a_package_says_so() {
    let home = home("not_a_package", &[]);
    let fake = home
        .join("native_packages")
        .join(format!("notsaule{}", std::env::consts::DLL_SUFFIX));
    std::fs::write(&fake, b"not a shared library").expect("write the fake library");
    let out = saule(&home, "check", "import X from \"notsaule\"\n");
    let err = stderr(&out);
    assert!(err.contains("`notsaule` cannot be imported"), "{err}");
    assert!(err.contains("is not a shared library"), "{err}");
}

/// A package whose metadata declares another ABI is refused before anything
/// loads it, naming both versions.
#[test]
fn a_package_for_another_abi_is_refused_before_loading() {
    let ours = saule_native_abi::ABI_VERSION;
    let theirs = ours + 7;
    let mut bytes = std::fs::read(fixture_library()).expect("read the fixture");
    let from = format!("abi_version = {ours}\n");
    let to = format!("abi_version = {theirs}\n");
    assert_eq!(from.len(), to.len(), "the patch must not move any bytes");
    let at = bytes
        .windows(from.len())
        .position(|w| w == from.as_bytes())
        .expect("the package record declares its ABI");
    bytes[at..at + to.len()].copy_from_slice(to.as_bytes());

    let home = home("other_abi", &[]);
    let file = fixture_library().file_name().expect("file name");
    std::fs::write(home.join("native_packages").join(file), bytes).expect("install");

    let out = saule(
        &home,
        "run",
        "import * from \"fixture\"\nprintln(Counter.live())\n",
    );
    let err = stderr(&out);
    assert!(!out.status.success());
    assert!(
        err.contains(&format!("native ABI version {theirs}")),
        "{err}"
    );
    assert!(err.contains(&format!("speaks version {ours}")), "{err}");
}

#[test]
fn a_package_installed_the_old_way_explains_the_new_layout() {
    let home = home("legacy", &[]);
    let manifests = home.join("native_manifests");
    std::fs::create_dir_all(&manifests).expect("create the old manifest directory");
    std::fs::write(
        manifests.join("engine.toml"),
        "[package]\nname = \"engine\"\nversion = \"0.1.0\"\n",
    )
    .expect("write an old manifest");
    let out = saule(&home, "check", "import Timer from \"engine\"\n");
    let err = stderr(&out);
    assert!(err.contains("installed the old way"), "{err}");
}

#[test]
fn two_libraries_claiming_one_package_are_both_refused() {
    let lib = fixture_library();
    let home = home(
        "duplicate",
        &[
            (lib, &format!("a{}", std::env::consts::DLL_SUFFIX)),
            (lib, &format!("b{}", std::env::consts::DLL_SUFFIX)),
        ],
    );
    let out = saule(&home, "check", "import * from \"fixture\"\n");
    let err = stderr(&out);
    assert!(err.contains("is installed more than once"), "{err}");
}
