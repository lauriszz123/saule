//! `saule test` and the stack trace on a runtime error, through the binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("test_command")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create the scratch root");
    root
}

fn write(root: &Path, rel: &str, body: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    path
}

fn saule(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_saule"))
        .args(args)
        .current_dir(dir)
        .env("SAULE_HOME", dir.join(".saule-home"))
        .env("NO_COLOR", "1")
        .output()
        .expect("run saule")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn project(root: &Path) {
    write(
        root,
        "saule.config",
        "name: \"t\"\nversion: \"0.1.0\"\nentry: \"src/main.sau\"\nsrc_dirs: [\"src\"]\n",
    );
    write(
        root,
        "src/calc.sau",
        "export fn add(a: integer, b: integer) -> integer\n  return a + b\nend\n",
    );
    write(
        root,
        "src/main.sau",
        "class Main\n  static fn main()\n    println(\"the program itself\")\n  end\nend\n",
    );
}

#[test]
fn passing_tests_exit_zero() {
    let root = scratch("passing");
    project(&root);
    write(
        &root,
        "tests/calc_tests.sau",
        "import add from \"calc\"\n\nfn test_adds()\n  assert(add(1, 2) == 3)\nend\n\n\
         fn test_adds_again()\n  assert(add(2, 2) == 4)\nend\n",
    );
    let out = saule(&root, &["test"]);
    let shown = text(&out);
    assert!(out.status.success(), "{shown}");
    assert!(shown.contains("ok    test_adds\n"), "{shown}");
    assert!(shown.contains("2 passed, 0 failed"), "{shown}");
    assert!(
        !shown.contains("the program itself"),
        "main must not run: {shown}"
    );
}

#[test]
fn a_failure_is_reported_and_the_rest_still_run() {
    let root = scratch("failing");
    project(&root);
    write(
        &root,
        "tests/mixed.sau",
        "fn test_fails()\n  assert(1 == 2, \"one is not two\")\nend\n\n\
         fn test_throws()\n  throw \"boom\"\nend\n\n\
         fn test_faults()\n  local z: integer = 0\n  println(1 / z)\nend\n\n\
         fn test_passes()\n  assert(true)\nend\n",
    );
    let out = saule(&root, &["test"]);
    let shown = text(&out);
    assert_eq!(out.status.code(), Some(1), "{shown}");
    assert!(shown.contains("one is not two"), "{shown}");
    assert!(shown.contains("boom"), "{shown}");
    assert!(shown.contains("division by zero"), "{shown}");
    assert!(shown.contains("ok    test_passes"), "{shown}");
    assert!(shown.contains("1 passed, 3 failed"), "{shown}");
}

#[test]
fn a_test_can_be_found_beside_the_source_and_filtered() {
    let root = scratch("filter");
    project(&root);
    write(
        &root,
        "src/calc_test.sau",
        "fn test_wanted()\n  assert(true)\nend\n\nfn test_other()\n  assert(false)\nend\n",
    );
    let out = saule(&root, &["test", "--filter", "wanted"]);
    let shown = text(&out);
    assert!(out.status.success(), "{shown}");
    assert!(shown.contains("1 passed, 0 failed"), "{shown}");
}

#[test]
fn a_file_that_does_not_compile_is_a_failure() {
    let root = scratch("broken");
    project(&root);
    write(
        &root,
        "tests/broken.sau",
        "fn test_a()\n  local x: integer = \"s\"\nend\n",
    );
    let out = saule(&root, &["test"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("does not compile"));
}

#[test]
fn no_tests_is_not_a_failure() {
    let root = scratch("none");
    project(&root);
    let out = saule(&root, &["test"]);
    assert!(out.status.success());
    assert!(text(&out).contains("No tests found"));
}

const CHAIN: &str = "fn divide(a: integer, b: integer) -> integer\n  return a / b\nend\n\n\
fn middle() -> integer\n  local r: integer = divide(1, 0)\n  return r\nend\n\n\
class Runner\n  static fn go() -> integer\n    local r: integer = middle()\n    return r\n  end\nend\n\n\
println(Runner.go())\n";

#[test]
fn a_runtime_error_shows_how_the_program_got_there() {
    let root = scratch("trace");
    let file = write(&root, "t.sau", CHAIN);
    let out = saule(&root, &["run", file.to_str().unwrap()]);
    let shown = text(&out);
    assert_eq!(out.status.code(), Some(1), "{shown}");
    assert!(shown.contains("division by zero"), "{shown}");
    assert!(shown.contains("stack trace"), "{shown}");
    let divide = shown.find("at divide").expect("divide");
    let middle = shown.find("at middle (t.sau:6)").expect(&shown);
    let go = shown.find("(t.sau:12)").expect(&shown);
    assert!(divide < middle && middle < go, "innermost first: {shown}");
}

#[test]
fn an_uncaught_throw_shows_its_trace_and_a_caught_one_leaves_none() {
    let root = scratch("throw");
    let file = write(
        &root,
        "t.sau",
        "fn inner()\n  throw \"caught\"\nend\n\nfn safe()\n  try\n    inner()\n  catch e: string\n    println(e)\n  end\nend\n\n\
         fn thrower()\n  throw \"boom\"\nend\n\nfn outer()\n  safe()\n  thrower()\n  println(\"unreachable\")\nend\n\nouter()\n",
    );
    let out = saule(&root, &["run", file.to_str().unwrap()]);
    let shown = text(&out);
    assert_eq!(out.status.code(), Some(1), "{shown}");
    assert!(shown.contains("thrower"), "{shown}");
    assert!(shown.contains("outer"), "{shown}");
    let trace = &shown[shown.find("stack trace").expect(&shown)..];
    assert!(!trace.contains("inner"), "a caught throw is over: {trace}");
}

#[test]
fn a_failure_with_no_calls_needs_no_trace() {
    let root = scratch("shallow");
    let file = write(&root, "t.sau", "local z: integer = 0\nprintln(1 / z)\n");
    let out = saule(&root, &["run", file.to_str().unwrap()]);
    assert!(!text(&out).contains("stack trace"), "{}", text(&out));
}

#[test]
fn runaway_recursion_reports_a_bounded_trace() {
    let root = scratch("deep");
    let file = write(
        &root,
        "t.sau",
        "fn down(n: integer) -> integer\n  local r: integer = down(n + 1)\n  return r\nend\n\nprintln(down(0))\n",
    );
    let out = saule(&root, &["run", file.to_str().unwrap()]);
    let shown = text(&out);
    assert_eq!(out.status.code(), Some(1), "{shown}");
    assert!(
        shown.lines().count() < 120,
        "{} lines",
        shown.lines().count()
    );
    assert!(shown.contains("more"), "{shown}");
}
