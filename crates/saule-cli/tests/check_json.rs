//! `saule check --json`, through the binary a user runs.
//!
//! The point of the flag is that a *program* reads the output, so these tests
//! parse it as JSON rather than matching substrings — the same reason the flag
//! exists at all. A test that asserted on rendered text would be the screen
//! scraping this feature is meant to replace.
//!
//! Each test writes its fixtures under `CARGO_TARGET_TMPDIR`, so nothing here
//! touches the repository's own `tests/` tree.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// A scratch directory for one test.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("check_json")
            .join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create the scratch root");
        Scratch { root }
    }

    /// Write a file, creating parents, and return its path.
    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create a parent directory");
        }
        std::fs::write(&path, body).expect("write a fixture");
        path
    }
}

/// Outcome of one `saule check` invocation.
struct Run {
    stdout: String,
    status: Option<i32>,
}

impl Run {
    /// Parse stdout as JSON, failing with the raw text when it is not.
    ///
    /// This assertion is the contract: whatever happens, `--json` puts one
    /// JSON document on stdout and nothing else.
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout was not JSON ({e}): {:?}", self.stdout))
    }

    /// Every diagnostic across every file, flattened.
    fn diagnostics(&self) -> Vec<Value> {
        self.json()["files"]
            .as_array()
            .expect("files is an array")
            .iter()
            .flat_map(|f| {
                f["diagnostics"]
                    .as_array()
                    .expect("diagnostics is an array")
                    .clone()
            })
            .collect()
    }
}

fn check(args: &[&Path], json: bool) -> Run {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_saule"));
    cmd.arg("check");
    for a in args {
        cmd.arg(a);
    }
    if json {
        cmd.arg("--json");
    }
    let out = cmd.output().expect("run saule check");
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        status: out.status.code(),
    }
}

fn check_json(target: &Path) -> Run {
    check(&[target], true)
}

#[test]
fn a_clean_file_reports_ok_and_exits_zero() {
    let s = Scratch::new("clean");
    let f = s.write("ok.sau", "local n: integer = 1\nprintln(n)\n");

    let run = check_json(&f);
    let doc = run.json();

    assert_eq!(doc["ok"], true);
    assert_eq!(doc["summary"]["files"], 1);
    assert_eq!(doc["summary"]["errors"], 0);
    assert_eq!(doc["summary"]["files_with_errors"], 0);
    // The file is still listed. A consumer needs to know what was examined
    // and found clean, not only what complained.
    assert_eq!(doc["files"].as_array().unwrap().len(), 1);
    assert!(run.diagnostics().is_empty());
    assert_eq!(run.status, Some(0));
}

#[test]
fn a_type_error_carries_its_phase_span_and_help() {
    let s = Scratch::new("type_error");
    let source = "local n: integer = 3.14\n";
    let f = s.write("bad.sau", source);

    let run = check_json(&f);
    assert_eq!(run.json()["ok"], false);
    assert_eq!(run.status, Some(1));

    let diags = run.diagnostics();
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    let d = &diags[0];

    assert_eq!(d["severity"], "error");
    assert_eq!(d["phase"], "type");
    assert!(
        d["message"].as_str().unwrap().contains("float"),
        "message should name the offending type: {d}"
    );

    // The span must be byte offsets into the source that actually bracket the
    // offending expression — a span that merely exists is no use to a caller
    // splicing a fix.
    let start = d["span"]["start"].as_u64().unwrap() as usize;
    let end = d["span"]["end"].as_u64().unwrap() as usize;
    assert!(start < end, "span must be non-empty: {d}");
    assert_eq!(&source[start..end], "3.14");
}

#[test]
fn a_lex_error_is_phase_lex() {
    let s = Scratch::new("lex_error");
    let f = s.write("lex.sau", "local s: string = \"unterminated\n");

    let run = check_json(&f);
    let diags = run.diagnostics();
    assert!(!diags.is_empty(), "expected a diagnostic");
    assert_eq!(diags[0]["phase"], "lex");
    assert_eq!(run.status, Some(1));
}

#[test]
fn a_parse_error_is_phase_parse() {
    let s = Scratch::new("parse_error");
    let f = s.write("parse.sau", "local x: integer =\n");

    let run = check_json(&f);
    let diags = run.diagnostics();
    assert!(!diags.is_empty(), "expected a diagnostic");
    assert_eq!(diags[0]["phase"], "parse");
    assert_eq!(run.status, Some(1));
}

/// An unresolved `import` is its own phase: the fix is a missing file or
/// dependency, not a mistake in the code, and a caller may well want to treat
/// the two differently.
#[test]
fn an_unresolved_import_is_phase_import() {
    let s = Scratch::new("bad_import");
    let f = s.write("imp.sau", "import * from \"nowhere\"\n");

    let run = check_json(&f);
    let diags = run.diagnostics();
    assert!(!diags.is_empty(), "expected a diagnostic");
    assert_eq!(diags[0]["phase"], "import");
    assert_eq!(run.status, Some(1));
}

/// A missing file still produces a parseable document. Without this, a harness
/// reading stdout gets nothing to parse and cannot tell a crash from a clean
/// run.
#[test]
fn a_missing_file_still_yields_json_on_stdout() {
    let s = Scratch::new("missing");
    let absent = s.root.join("does_not_exist.sau");

    let run = check_json(&absent);
    let doc = run.json();

    assert_eq!(doc["ok"], false);
    let diags = run.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0]["phase"], "io",
        "a missing file is an I/O problem, not an import error: {}",
        diags[0]
    );
    assert_eq!(run.status, Some(1));
}

/// Project mode checks every file under `src_dirs`, and the JSON lists all of
/// them — clean ones included — so a consumer can see the whole run.
#[test]
fn project_mode_lists_every_file_checked() {
    let s = Scratch::new("project");
    s.write(
        "saule.config",
        "name: \"probe\"\nversion: \"0.1.0\"\nkind: \"library\"\nsrc_dirs: [\"src\"]\n",
    );
    s.write(
        "src/good.sau",
        "export class Good\n  static fn one()\n  end\nend\n",
    );
    s.write("src/bad.sau", "local n: integer = 3.14\n");

    let run = check(&[&s.root], true);
    let doc = run.json();

    assert_eq!(doc["ok"], false);
    assert_eq!(doc["summary"]["files"], 2, "both files examined: {doc}");
    assert_eq!(doc["summary"]["files_with_errors"], 1);
    assert_eq!(doc["summary"]["errors"], 1);

    // Each entry names its own path, which is what distinguishes two files
    // that happen to share a basename.
    let paths: Vec<&str> = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("good.sau")),
        "clean file should be listed: {paths:?}"
    );
    assert!(paths.iter().any(|p| p.ends_with("bad.sau")), "{paths:?}");
    assert_eq!(run.status, Some(1));
}

/// A stored verdict is only meaningful next to the compiler that issued it,
/// and a binary on `PATH` can lag the tree it was built from.
#[test]
fn the_document_records_which_compiler_judged() {
    let s = Scratch::new("provenance");
    let f = s.write("ok.sau", "local n: integer = 1\n");

    let doc = check_json(&f).json();

    let version = doc["saule_version"].as_str().expect("a version string");
    assert!(!version.is_empty());

    // Must agree with what the binary reports for itself, or the field is
    // worse than absent.
    let reported = Command::new(env!("CARGO_BIN_EXE_saule"))
        .arg("--version")
        .output()
        .expect("run saule --version");
    let reported = String::from_utf8_lossy(&reported.stdout).into_owned();
    assert!(
        reported.contains(version),
        "`--version` said {reported:?}, the document said {version:?}"
    );

    assert!(!doc["commit"].as_str().expect("a commit string").is_empty());
}

/// `--json` replaces the rendered output rather than adding to it: mixing
/// miette's box drawing into stdout would defeat the whole flag.
#[test]
fn json_mode_puts_nothing_but_json_on_stdout() {
    let s = Scratch::new("stdout_only");
    let f = s.write("bad.sau", "local n: integer = 3.14\n");

    let run = check_json(&f);

    // Parses as exactly one document, with no leading or trailing noise.
    let trimmed = run.stdout.trim();
    assert!(trimmed.starts_with('{'), "stdout: {:?}", run.stdout);
    assert!(trimmed.ends_with('}'), "stdout: {:?}", run.stdout);
    run.json();
    assert!(
        !run.stdout.contains('×'),
        "the rendered form leaked onto stdout: {:?}",
        run.stdout
    );
}

/// Without the flag, nothing changes: stdout stays the human summary.
#[test]
fn without_the_flag_stdout_is_unchanged() {
    let s = Scratch::new("human");
    let f = s.write("ok.sau", "local n: integer = 1\n");

    let run = check(&[&f], false);

    assert!(
        run.stdout.contains("no errors"),
        "expected the human summary: {:?}",
        run.stdout
    );
    assert!(serde_json::from_str::<Value>(&run.stdout).is_err());
    assert_eq!(run.status, Some(0));
}

/// Every diagnostic in one pass, as `check` has always promised — the JSON
/// must not quietly report only the first.
#[test]
fn every_error_is_reported_not_just_the_first() {
    let s = Scratch::new("all_errors");
    let f = s.write(
        "many.sau",
        "local a: integer = 1.5\nlocal b: integer = 2.5\nlocal c: integer = 3.5\n",
    );

    let run = check_json(&f);
    let diags = run.diagnostics();
    assert!(
        diags.len() >= 3,
        "expected all three, got {}: {diags:?}",
        diags.len()
    );
    assert_eq!(run.json()["summary"]["errors"], diags.len());
}

/// Absent optionals are omitted rather than serialized as `null`, so a
/// consumer can test for presence with a plain key check.
#[test]
fn absent_optional_fields_are_omitted() {
    let s = Scratch::new("omitted");
    let f = s.write("lex.sau", "local s: string = \"unterminated\n");

    let diags = check_json(&f).diagnostics();
    let d = diags[0].as_object().unwrap();

    // No error variant declares a `#[diagnostic(code(...))]` yet, so this key
    // must be absent — not present and null. When codes land, this test is
    // the one that says so.
    assert!(!d.contains_key("code"), "unexpected code: {d:?}");
    assert!(d.contains_key("severity"));
    assert!(d.contains_key("phase"));
    assert!(d.contains_key("message"));
}
