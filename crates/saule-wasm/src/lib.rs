//! The Saule interpreter, compiled for the browser.
//!
//! One entry point, [`run_to_json`], which takes a program's source and
//! returns a JSON string matching the `RunResult` shape declared in
//! `www/src/lib/runtime.ts`:
//!
//! ```json
//! {
//!   "output": [{ "stream": "stdout", "text": "hello\n" }],
//!   "diagnostics": [],
//!   "ok": true
//! }
//! ```
//!
//! (`durationMs` is deliberately absent — the page times the call itself, so
//! the measurement includes module-call overhead the interpreter cannot see.)
//!
//! ## Why this crate exists separately
//!
//! `saule-interpreter` stays free of `wasm-bindgen` and `js-sys`. It exposes
//! two seams instead — [`output::Sink`] for where `print` goes and
//! [`platform::Platform`] for clocks and process control — and this crate
//! fills them with browser implementations. That is what lets the interpreter
//! keep building for every other target unchanged, and it is why the JSON
//! shaping below can be unit-tested natively with no browser involved.
//!
//! ## Sandbox
//!
//! Programs run in **single-file mode**: top-to-bottom, no `class Main`
//! required. `import` is unavailable because module resolution needs a
//! filesystem — a program that tries gets a plain `import error` diagnostic
//! rather than a crash.
//!
//! [`output::Sink`]: saule_interpreter::output::Sink
//! [`platform::Platform`]: saule_interpreter::platform::Platform

use serde::Serialize;

use saule_interpreter::output::{self, Stream};
use saule_interpreter::platform;

// ─── the JSON contract ─────────────────────────────────────────────────────

/// Byte offsets into the source, so the editor can underline the exact range
/// rather than printing a bare message.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// Which pass produced a diagnostic. Surfaced as a label in the output pane.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Lex,
    Parse,
    Semantic,
    Type,
    Runtime,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Diagnostic {
    /// Always `"error"` today. The field exists because the shape allows
    /// warnings and the front end already renders them differently.
    pub severity: &'static str,
    pub phase: Phase,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct OutputChunk {
    pub stream: &'static str,
    pub text: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RunResult {
    pub output: Vec<OutputChunk>,
    pub diagnostics: Vec<Diagnostic>,
    pub ok: bool,
}

// ─── diagnostic mapping ────────────────────────────────────────────────────

/// Turn any of the pipeline's error types into a [`Diagnostic`].
///
/// Every error in the toolchain derives `miette::Diagnostic` and carries a
/// `#[label]`, so the span and help text come off the trait rather than out of
/// a match over each enum. A new error variant therefore needs no change here.
fn to_diagnostic<E>(err: &E, phase: Phase) -> Diagnostic
where
    E: std::error::Error + miette::Diagnostic,
{
    let span = err
        .labels()
        .and_then(|mut labels| labels.next())
        .map(|label| Span {
            start: label.offset(),
            end: label.offset() + label.len(),
        });

    Diagnostic {
        severity: "error",
        phase,
        message: err.to_string(),
        span,
        help: err.help().map(|h| h.to_string()),
    }
}

fn failed(diagnostics: Vec<Diagnostic>, output: Vec<OutputChunk>) -> RunResult {
    RunResult {
        output,
        diagnostics,
        ok: false,
    }
}

// ─── the pipeline ──────────────────────────────────────────────────────────

/// Compile and run `source`, returning the structured result.
///
/// Unlike `saule_interpreter::check_and_run`, which stops at the first
/// diagnostic, this reports **every** semantic and type error in one pass —
/// a playground that fixes one error at a time and re-runs is a poor way to
/// learn a type system.
pub fn run(source: &str) -> RunResult {
    // Registers the stdlib's type signatures and prelude names. `check_and_run`
    // does this internally; running the phases by hand means doing it here.
    saule_interpreter::init();

    let tokens = match saule_lexer::Lexer::new(source).tokenize() {
        Ok(tokens) => tokens,
        Err(err) => return failed(vec![to_diagnostic(&err, Phase::Lex)], Vec::new()),
    };

    let module = match saule_parser::parse(tokens) {
        Ok(module) => module,
        Err(err) => return failed(vec![to_diagnostic(&err, Phase::Parse)], Vec::new()),
    };

    // No import seed: resolving imports needs a filesystem, and there isn't
    // one. A program that imports gets a normal diagnostic from the loader.
    let semantic: Vec<Diagnostic> = saule_semantic::analyze(&module)
        .iter()
        .map(|e| to_diagnostic(e, Phase::Semantic))
        .collect();
    if !semantic.is_empty() {
        return failed(semantic, Vec::new());
    }

    let type_errors: Vec<Diagnostic> = saule_typeck::check(&module)
        .iter()
        .map(|e| to_diagnostic(e, Phase::Type))
        .collect();
    if !type_errors.is_empty() {
        return failed(type_errors, Vec::new());
    }

    // Anything the program prints is captured rather than written to a stdout
    // that does not exist on this target.
    let (sink, outcome) = output::capture(|| saule_interpreter::run(&module));

    let output: Vec<OutputChunk> = sink
        .chunks()
        .into_iter()
        .map(|c| OutputChunk {
            stream: match c.stream {
                Stream::Stdout => "stdout",
                Stream::Stderr => "stderr",
            },
            text: c.text,
        })
        .collect();

    match outcome {
        Ok(_) => RunResult {
            output,
            diagnostics: Vec::new(),
            ok: true,
        },
        Err(err) => {
            // `Os.exit(n)` unwinds rather than terminating the module, so it
            // arrives here as an error. It is a normal way for a program to
            // finish, not a fault — report it as success and keep the output.
            if platform::take_exit().is_some() {
                return RunResult {
                    output,
                    diagnostics: Vec::new(),
                    ok: true,
                };
            }
            failed(vec![to_diagnostic(&err, Phase::Runtime)], output)
        }
    }
}

/// [`run`], serialized. The browser entry point returns this string.
///
/// Serialization cannot realistically fail — every field is a plain string,
/// number or bool — but if it somehow did, returning a hand-built error
/// document beats panicking, which on wasm aborts the whole module.
pub fn run_to_json(source: &str) -> String {
    let result = run(source);
    serde_json::to_string(&result).unwrap_or_else(|e| {
        format!(
            r#"{{"output":[],"diagnostics":[{{"severity":"error","phase":"runtime","message":"could not serialize the run result: {}"}}],"ok":false}}"#,
            e.to_string().replace('"', "'")
        )
    })
}

// ─── browser bindings ──────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod browser {
    use wasm_bindgen::prelude::*;

    /// Wall-clock and elapsed time, from JavaScript.
    ///
    /// `Date::now` rather than `performance.now()` for both: it needs no
    /// `web-sys` dependency, and it is available in a Worker as readily as on
    /// the main thread. `Os.clock` only needs elapsed seconds at human
    /// resolution, so the loss of monotonicity does not matter here.
    struct BrowserPlatform {
        start_ms: f64,
    }

    impl saule_interpreter::platform::Platform for BrowserPlatform {
        fn unix_time_secs(&self) -> Option<f64> {
            Some(js_sys::Date::now() / 1000.0)
        }

        fn monotonic_secs(&self) -> Option<f64> {
            Some((js_sys::Date::now() - self.start_ms) / 1000.0)
        }

        // `sleep` and `pid` keep the trait's "unavailable" defaults: a browser
        // cannot block, and a module has no process. `exit` likewise, so
        // `Os.exit` unwinds instead of tearing the module down.
    }

    /// Compile and run a Saule program, returning a JSON `RunResult`.
    #[wasm_bindgen]
    pub fn run(source: &str) -> String {
        let platform = BrowserPlatform {
            start_ms: js_sys::Date::now(),
        };
        saule_interpreter::platform::with_platform(Box::new(platform), || {
            super::run_to_json(source)
        })
    }

    /// The toolchain version this module was built from, so the playground can
    /// show which release it is running.
    #[wasm_bindgen]
    pub fn version() -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host that can do nothing — which is precisely what `BrowserPlatform`
    /// is for the facilities it does not override, `exit` among them.
    ///
    /// Without this, tests covering `Os.exit` would call the *native*
    /// platform's `exit`, i.e. `std::process::exit`, and take the test binary
    /// down with them. That the native build really does terminate is correct
    /// behaviour; it just isn't the behaviour under test.
    struct Sandbox;
    impl saule_interpreter::platform::Platform for Sandbox {}

    fn json(source: &str) -> serde_json::Value {
        let raw = saule_interpreter::platform::with_platform(Box::new(Sandbox), || {
            run_to_json(source)
        });
        serde_json::from_str(&raw).expect("valid JSON")
    }

    #[test]
    fn a_working_program_reports_its_output_and_ok() {
        let v = json(r#"println("hello, world!")"#);
        assert_eq!(v["ok"], true);
        assert_eq!(v["diagnostics"].as_array().unwrap().len(), 0);
        assert_eq!(v["output"][0]["stream"], "stdout");
        assert_eq!(v["output"][0]["text"], "hello, world!\n");
    }

    #[test]
    fn stdout_and_stderr_stay_distinguishable() {
        let v = json("println(\"out\") Io.stderr.write(\"err\")");
        let chunks = v["output"].as_array().unwrap();
        assert_eq!(chunks[0]["stream"], "stdout");
        assert_eq!(chunks[1]["stream"], "stderr");
        assert_eq!(chunks[1]["text"], "err");
    }

    #[test]
    fn a_lex_error_is_reported_with_its_phase_and_span() {
        let v = json(r#"local s: string = "unterminated"#);
        assert_eq!(v["ok"], false);
        let d = &v["diagnostics"][0];
        assert_eq!(d["phase"], "lex");
        assert_eq!(d["severity"], "error");
        assert!(d["span"]["start"].is_number(), "expected a span: {d}");
    }

    #[test]
    fn a_parse_error_is_reported() {
        let v = json("local x: integer =");
        assert_eq!(v["ok"], false);
        assert_eq!(v["diagnostics"][0]["phase"], "parse");
    }

    #[test]
    fn a_type_error_is_reported_with_a_span() {
        let v = json("local n: integer = 3.14");
        assert_eq!(v["ok"], false);
        let d = &v["diagnostics"][0];
        assert_eq!(d["phase"], "type");
        let span = &d["span"];
        assert!(span["end"].as_u64().unwrap() > span["start"].as_u64().unwrap());
    }

    #[test]
    fn every_type_error_is_reported_not_just_the_first() {
        // A playground that surfaces one error per run is a poor way to learn
        // a type system.
        let v = json(
            r#"
local a: integer = 1.5
local b: integer = 2.5
local c: integer = 3.5
"#,
        );
        assert_eq!(v["ok"], false);
        assert!(
            v["diagnostics"].as_array().unwrap().len() >= 3,
            "expected all three, got {}",
            v["diagnostics"]
        );
    }

    #[test]
    fn a_runtime_error_keeps_the_output_printed_before_it() {
        let v = json(
            r#"
println("before the failure")
local zero: integer = 0
println(1 / zero)
"#,
        );
        assert_eq!(v["ok"], false);
        assert_eq!(v["diagnostics"][0]["phase"], "runtime");
        // Output produced before the fault must survive — otherwise a program
        // that crashes halfway looks like it never ran.
        assert_eq!(v["output"][0]["text"], "before the failure\n");
    }

    #[test]
    fn os_exit_counts_as_finishing_not_failing() {
        let v = json(r#"println("done") Os.exit(0)"#);
        assert_eq!(v["ok"], true, "Os.exit is a normal ending: {v}");
        assert_eq!(v["output"][0]["text"], "done\n");
    }

    #[test]
    fn a_nonzero_os_exit_is_also_a_clean_finish() {
        let v = json(r#"Os.exit(2)"#);
        assert_eq!(v["ok"], true);
        assert_eq!(v["diagnostics"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn import_fails_with_a_diagnostic_rather_than_crashing() {
        // No filesystem in the sandbox. This must be an ordinary error.
        let v = json(r#"import Foo from "bar""#);
        assert_eq!(v["ok"], false);
        assert!(!v["diagnostics"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_program_printing_nothing_yields_an_empty_output_array() {
        let v = json("local unused: integer = 1");
        assert_eq!(v["ok"], true);
        assert_eq!(v["output"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn help_text_is_carried_through_when_present() {
        // Force-unwrapping nil has a `#[diagnostic(help(...))]` attached.
        let v = json(
            r#"
local maybe: string? = nil
println(maybe!)
"#,
        );
        assert_eq!(v["ok"], false);
        assert!(
            v["diagnostics"][0]["help"].is_string(),
            "expected help text: {}",
            v["diagnostics"][0]
        );
    }

    #[test]
    fn classes_and_pattern_matching_run_end_to_end() {
        let v = json(
            r#"
enum Shape
    Circle(radius: float),
    Rect(w: float, h: float)
end

fn area(s: Shape) -> float
    return match s
        case Shape.Circle(r) then 3.0 * r * r
        case Shape.Rect(w, h) then w * h
    end
end

println(area(Shape.Rect(3.0, 4.0)))
"#,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["output"][0]["text"], "12.0\n");
    }

    #[test]
    fn the_json_omits_absent_optional_fields() {
        // `span` and `help` are optional in the TS interface; emitting nulls
        // would make `if (d.span)` checks pass on a null.
        let raw = run_to_json(r#"println("hi")"#);
        assert!(!raw.contains("null"), "unexpected null in {raw}");
    }
}
