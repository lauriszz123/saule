//! `Os` behaves sanely on a host with no clock, no sleep and no process.
//!
//! On `wasm32-unknown-unknown` the calls these functions used to make —
//! `SystemTime::now()`, `Instant::now()`, `thread::sleep`,
//! `std::process::exit` — panic or tear the module down. A one-line program
//! calling `Os.time()` could therefore kill the playground.
//!
//! These tests install a `Platform` that reports every facility as
//! unavailable, which is exactly what a bare wasm host looks like, and pin
//! the resulting behaviour. They run on any target.

use saule_interpreter::platform::{self, Platform};

/// A host that can do nothing — the wasm default, modelled explicitly so
/// these assertions hold when run natively.
struct Sandbox;
impl Platform for Sandbox {}

/// A host with a clock but no process, like a browser with `Date.now()`.
struct BrowserLike;
impl Platform for BrowserLike {
    fn unix_time_secs(&self) -> Option<f64> {
        Some(1_700_000_000.0)
    }
    fn monotonic_secs(&self) -> Option<f64> {
        Some(2.5)
    }
}

fn run(src: &str) -> Result<String, String> {
    let tokens = saule_lexer::Lexer::new(src)
        .tokenize()
        .map_err(|e| e.to_string())?;
    let module = saule_parser::parse(tokens).map_err(|e| e.to_string())?;
    let (sink, result) =
        saule_interpreter::output::capture(|| saule_interpreter::check_and_run(&module));
    match result {
        Ok(_) => Ok(sink.text()),
        Err(e) => Err(e.to_string()),
    }
}

fn run_sandboxed(src: &str) -> Result<String, String> {
    platform::with_platform(Box::new(Sandbox), || run(src))
}

#[test]
fn os_time_reports_unavailable_instead_of_panicking() {
    let err = run_sandboxed("println(Os.time())").unwrap_err();
    assert!(
        err.contains("Os.time") && err.contains("unavailable"),
        "expected a clear unavailable error, got: {err}"
    );
}

#[test]
fn os_clock_reports_unavailable_instead_of_panicking() {
    let err = run_sandboxed("println(Os.clock())").unwrap_err();
    assert!(
        err.contains("Os.clock") && err.contains("unavailable"),
        "got: {err}"
    );
}

#[test]
fn os_sleep_reports_unavailable_rather_than_silently_not_sleeping() {
    let err = run_sandboxed("Os.sleep(1)").unwrap_err();
    assert!(err.contains("Os.sleep"), "got: {err}");
}

#[test]
fn os_exit_unwinds_and_records_its_code_instead_of_terminating() {
    // Reaching the assertion at all proves the process was not killed.
    let _ = platform::take_exit();
    let err = platform::with_platform(Box::new(Sandbox), || {
        run("println(\"before\") Os.exit(3) println(\"after\")").unwrap_err()
    });

    assert!(err.contains("exited with code 3"), "got: {err}");
    assert_eq!(platform::take_exit(), Some(3));
}

#[test]
fn os_pid_is_zero_rather_than_an_error() {
    // Informational only, so it stays total — programs use it for uniqueness.
    let out = run_sandboxed("println(Os.pid())").expect("should not error");
    assert_eq!(out, "0\n");
}

#[test]
fn os_tmpname_still_works_without_a_clock_or_pid() {
    let out = run_sandboxed("println(Os.tmpname()) println(Os.tmpname())")
        .expect("tmpname must stay total");
    let names: Vec<&str> = out.lines().collect();
    assert_eq!(names.len(), 2);
    assert_ne!(names[0], names[1], "successive names must differ");
}

#[test]
fn os_date_with_an_explicit_time_needs_no_clock() {
    // Only the implicit "now" requires a clock; formatting a supplied instant
    // is pure arithmetic and must work anywhere.
    let out = run_sandboxed(r#"println(Os.date("%Y", 0))"#).expect("should not error");
    assert_eq!(out, "1970\n");
}

#[test]
fn os_date_without_a_time_reports_unavailable() {
    let err = run_sandboxed(r#"println(Os.date("%Y"))"#).unwrap_err();
    assert!(err.contains("Os.date"), "got: {err}");
}

#[test]
fn an_injected_clock_makes_time_and_clock_work() {
    let out = platform::with_platform(Box::new(BrowserLike), || {
        run("println(Os.time()) println(Os.clock())").expect("should not error")
    });
    assert_eq!(out, "1700000000\n2.5\n");
}

#[test]
fn a_program_using_no_host_facilities_is_unaffected() {
    let out = run_sandboxed(
        r#"
local total: integer = 0
for i = 1, 5 do
    total = total + i
end
println("total = " .. total)
"#,
    )
    .expect("pure computation must not need a host");
    assert_eq!(out, "total = 15\n");
}
