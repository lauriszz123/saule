//! The Saule language, compiled for the browser.
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
//! ## How the program runs
//!
//! Compiled to bytecode and run on the VM, exactly as `saule run` does.
//!
//! ## Sandbox
//!
//! Programs run in **single-file mode**: top-to-bottom, no `class Main`
//! required. `import` is unavailable because module resolution needs a
//! filesystem — a program that tries gets a plain `import error` diagnostic
//! rather than a crash.
//!
//! [`output::Sink`]: saule_runtime::output::Sink
//! [`platform::Platform`]: saule_runtime::platform::Platform

use serde::Serialize;

use saule_runtime::output::{self, Stream};
use saule_runtime::platform;

// ─── the JSON contract ─────────────────────────────────────────────────────

// The diagnostic half of the contract lives in `saule-diagnostics`, because
// `saule check --json` and the training harnesses emit the same shape and a
// second copy of it here would drift. Re-exported rather than merely used, so
// a consumer of this crate still names these types through it.
pub use saule_diagnostics::{Diagnostic, Phase, Severity, Span};

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

use saule_diagnostics::to_diagnostic;

fn failed(diagnostics: Vec<Diagnostic>, output: Vec<OutputChunk>) -> RunResult {
    RunResult {
        output,
        diagnostics,
        ok: false,
    }
}

// ─── the pipeline ──────────────────────────────────────────────────────────

/// The chunk name the playground compiles under. There is no file behind
/// this source, but a chunk carries a name for its disassembly and errors.
const PLAYGROUND_NAME: &str = "playground.sau";

/// What the front end produced: something runnable, or why not.
enum Front {
    Ready(std::rc::Rc<saule_vm::Chunk>),
    Failed(Vec<Diagnostic>),
}

/// Everything up to but not including execution.
///
/// Factored out because [`check`] wants exactly this and nothing more: the
/// playground reports errors as you type without running anything, and a
/// second copy of the phase order is how the two would come to disagree
/// about what a program means.
///
/// Unlike `saule_runtime::check_and_run`, which stops at the first
/// diagnostic, this reports **every** semantic and type error in one pass —
/// a playground that fixes one error at a time and re-runs is a poor way to
/// learn a type system.
fn front_end(source: &str) -> Front {
    // Registers the stdlib's type signatures and prelude names. `check_and_run`
    // does this internally; running the phases by hand means doing it here.
    saule_runtime::init();

    let tokens = match saule_lexer::Lexer::new(source).tokenize() {
        Ok(tokens) => tokens,
        Err(err) => return Front::Failed(vec![to_diagnostic(&err, Phase::Lex)]),
    };

    let mut module = match saule_parser::parse(tokens) {
        Ok(module) => module,
        Err(err) => return Front::Failed(vec![to_diagnostic(&err, Phase::Parse)]),
    };

    // The two static passes by hand, in the order `analyze_and_check`
    // enforces — analyse, then typecheck-and-resolve. Spelled out here
    // rather than delegated because that helper stops at the first
    // diagnostic and this one must report them all.
    //
    // No import seed: resolving imports needs a filesystem, and there isn't
    // one. A program that imports gets a compile error.
    let (sem_errors, _) =
        saule_runtime::analyze_with_bindings(&module, saule_semantic::ModuleSeed::default());
    let semantic: Vec<Diagnostic> = sem_errors
        .iter()
        .map(|e| to_diagnostic(e, Phase::Semantic))
        .collect();
    if !semantic.is_empty() {
        return Front::Failed(semantic);
    }

    let type_errors: Vec<Diagnostic> = saule_typeck::check_and_resolve(&mut module)
        .iter()
        .map(|e| to_diagnostic(e, Phase::Type))
        .collect();
    if !type_errors.is_empty() {
        return Front::Failed(type_errors);
    }

    // The bytecode engine — the only one. This is the single-module route
    // rather than `program::compile`, because there is no filesystem here
    // for an import graph to live in; an `import` is a compile error.
    match saule_vm::compile(&module, PLAYGROUND_NAME, source) {
        Ok(chunk) => Front::Ready(std::rc::Rc::new(chunk)),
        Err(err) => Front::Failed(vec![to_diagnostic(&err, Phase::Compile)]),
    }
}

/// Every compile-time diagnostic in `source`, without running it.
///
/// What the editor's squiggles are drawn from. Running a program to find out
/// whether it compiles is both slower and destructive — a half-finished
/// program that happens to parse would print, write and loop on every
/// keystroke.
pub fn check(source: &str) -> Vec<Diagnostic> {
    match front_end(source) {
        Front::Ready(_) => Vec::new(),
        Front::Failed(diagnostics) => diagnostics,
    }
}

/// Compile and run `source`, returning the structured result.
pub fn run(source: &str) -> RunResult {
    let chunk = match front_end(source) {
        Front::Ready(chunk) => chunk,
        Front::Failed(diagnostics) => return failed(diagnostics, Vec::new()),
    };

    // Anything the program prints is captured rather than written to a stdout
    // that does not exist on this target.
    let (sink, outcome) = output::capture(|| {
        saule_vm::run_chunk(chunk)
            .map(|vs| vs.into_iter().next().unwrap_or(saule_runtime::Value::Nil))
    });

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

/// The result of a [`format`] request.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FormatResult {
    pub ok: bool,
    /// The formatted source. Absent when the program did not parse.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Why it could not be formatted, as one diagnostic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<Diagnostic>,
}

/// Pretty-print `source`, preserving its comments.
///
/// The lossless lexer path, which is what `saule fmt` uses — the editor's
/// Format and the CLI's must not lay the same file out differently.
///
/// A program that does not parse is not formatted. There is nothing useful
/// to print from a broken tree, and a formatter that guesses at one would
/// hand back source the author never wrote.
pub fn format(source: &str) -> FormatResult {
    use saule_fmt::{Comment, CommentKind};
    use saule_lexer::Token;

    let raw = match saule_lexer::Lexer::new(source).tokenize_with_trivia() {
        Ok(raw) => raw,
        Err(err) => {
            return FormatResult {
                ok: false,
                text: None,
                diagnostic: Some(to_diagnostic(&err, Phase::Lex)),
            };
        }
    };

    let mut comments: Vec<Comment> = Vec::new();
    let mut tokens = Vec::with_capacity(raw.len());
    for tok in raw {
        match tok.value {
            Token::LineComment(text) => comments.push(Comment {
                span: tok.span,
                kind: CommentKind::Line,
                text,
            }),
            Token::BlockComment(text) => comments.push(Comment {
                span: tok.span,
                kind: CommentKind::Block,
                text,
            }),
            other => tokens.push(saule_ast::Spanned {
                value: other,
                span: tok.span,
                id: saule_ast::NodeId::NONE,
            }),
        }
    }

    match saule_parser::parse(tokens) {
        Ok(module) => FormatResult {
            ok: true,
            text: Some(saule_fmt::format_module_with_comments(
                &module, source, &comments,
            )),
            diagnostic: None,
        },
        Err(err) => FormatResult {
            ok: false,
            text: None,
            diagnostic: Some(to_diagnostic(&err, Phase::Parse)),
        },
    }
}

/// [`check`], serialized as `{"diagnostics": […]}`.
pub fn check_to_json(source: &str) -> String {
    #[derive(Serialize)]
    struct CheckResult {
        diagnostics: Vec<Diagnostic>,
    }
    let result = CheckResult {
        diagnostics: check(source),
    };
    serde_json::to_string(&result).unwrap_or_else(|_| r#"{"diagnostics":[]}"#.to_string())
}

/// [`format`], serialized.
pub fn format_to_json(source: &str) -> String {
    serde_json::to_string(&format(source))
        .unwrap_or_else(|_| r#"{"ok":false}"#.to_string())
}

/// One suggestion, in the shape the playground's editor consumes.
#[derive(Serialize, Debug)]
pub struct Completion {
    pub label: String,
    /// CodeMirror's vocabulary: `function`, `method`, `variable`, …
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Markdown, when the item is documented.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// Completions at byte `offset`, in the order the language server ranks them.
///
/// The same engine the editors use, so `p.` on a `Player` offers `Player`'s
/// members and a bare word offers the locals in scope — not every name the
/// library has. The ranking is carried by the order of the array.
pub fn complete(source: &str, offset: usize) -> Vec<Completion> {
    use lsp_types::{CompletionItemKind as K, Documentation};

    saule_runtime::init();
    // The engine slices the source at `offset`; one off a char boundary
    // would panic, which on wasm takes the whole module down.
    if offset > source.len() || !source.is_char_boundary(offset) {
        return Vec::new();
    }
    let mut items = saule_lsp::complete(source, offset);
    items.sort_by(|a, b| {
        let ka = a.sort_text.as_deref().unwrap_or(&a.label);
        let kb = b.sort_text.as_deref().unwrap_or(&b.label);
        ka.cmp(kb)
    });
    items
        .into_iter()
        .map(|i| Completion {
            kind: match i.kind {
                Some(K::METHOD) => "method",
                Some(K::FUNCTION | K::CONSTRUCTOR) => "function",
                Some(K::FIELD | K::PROPERTY) => "property",
                Some(K::VARIABLE | K::VALUE) => "variable",
                Some(K::CONSTANT) => "constant",
                Some(K::CLASS | K::STRUCT) => "class",
                Some(K::INTERFACE) => "interface",
                Some(K::ENUM) => "enum",
                Some(K::ENUM_MEMBER) => "enum",
                Some(K::MODULE | K::FILE) => "namespace",
                Some(K::KEYWORD) => "keyword",
                Some(K::SNIPPET) => "text",
                Some(K::TYPE_PARAMETER) => "type",
                _ => "text",
            },
            detail: i.detail,
            doc: i.documentation.map(|d| match d {
                Documentation::String(s) => s,
                Documentation::MarkupContent(m) => m.value,
            }),
            label: i.label,
        })
        .collect()
}

/// [`complete`], serialized as a JSON array.
pub fn complete_to_json(source: &str, offset: usize) -> String {
    serde_json::to_string(&complete(source, offset)).unwrap_or_else(|_| "[]".to_string())
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

    impl saule_runtime::platform::Platform for BrowserPlatform {
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
        saule_runtime::platform::with_platform(Box::new(platform), || super::run_to_json(source))
    }

    /// Report every compile-time diagnostic in a program without running it.
    ///
    /// No platform is installed, unlike [`run`]: nothing executes, so
    /// nothing can ask for a clock.
    #[wasm_bindgen]
    pub fn check(source: &str) -> String {
        super::check_to_json(source)
    }

    /// Completions at byte `offset`, as a JSON array of `Completion`s.
    #[wasm_bindgen]
    pub fn complete(source: &str, offset: usize) -> String {
        super::complete_to_json(source, offset)
    }

    /// Pretty-print a program, returning a JSON `FormatResult`.
    #[wasm_bindgen]
    pub fn format(source: &str) -> String {
        super::format_to_json(source)
    }

    /// The toolchain version this module was built from, so the playground can
    /// show which release it is running. The long form, because a playground
    /// built from an untagged commit should say so rather than claim to be the
    /// release it is heading toward.
    #[wasm_bindgen]
    pub fn version() -> String {
        saule_version::FULL.to_string()
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
    impl saule_runtime::platform::Platform for Sandbox {}

    fn json(source: &str) -> serde_json::Value {
        let raw = saule_runtime::platform::with_platform(Box::new(Sandbox), || run_to_json(source));
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
        let v = json(r#"from "bar" import Foo"#);
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
    fn the_playground_showcase_compiles() {
        // The program the playground opens with must compile — pinned on the
        // compile step itself, so a failure says where it broke.
        let source = r#"
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
"#;
        saule_runtime::init();
        let tokens = saule_lexer::Lexer::new(source).tokenize().expect("lexes");
        let module = saule_parser::parse(tokens).expect("parses");
        assert!(
            saule_runtime::analyze_with_bindings(&module, saule_semantic::ModuleSeed::default())
                .0
                .is_empty()
        );
        assert!(
            saule_vm::compile(&module, PLAYGROUND_NAME, source).is_ok(),
            "the playground's own showcase program must compile to bytecode"
        );
    }

    #[test]
    fn the_json_omits_absent_optional_fields() {
        // `span` and `help` are optional in the TS interface; emitting nulls
        // would make `if (d.span)` checks pass on a null.
        let raw = run_to_json(r#"println("hi")"#);
        assert!(!raw.contains("null"), "unexpected null in {raw}");
    }
}

#[cfg(test)]
mod check_and_format_tests {
    use super::*;

    #[test]
    fn a_clean_program_has_no_diagnostics() {
        assert_eq!(check("println(\"hi\")\n"), Vec::new());
    }

    #[test]
    fn check_reports_without_running() {
        // If this ran, the loop would never return.
        let diagnostics = check("local n: string = 1\nwhile true do end\n");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].phase, Phase::Type);
    }

    #[test]
    fn check_reports_every_type_error_at_once() {
        let diagnostics = check("local a: string = 1\nlocal b: integer = \"x\"\n");
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    }

    #[test]
    fn format_lays_out_and_keeps_comments() {
        let out = format("local   x:integer=1  -- note\n");
        assert!(out.ok);
        assert_eq!(out.text.as_deref(), Some("local x: integer = 1  -- note\n"));
    }

    #[test]
    fn format_declines_a_program_that_does_not_parse() {
        let out = format("local x: = \n");
        assert!(!out.ok);
        assert!(out.text.is_none());
        assert!(out.diagnostic.is_some());
    }

    #[test]
    fn format_does_not_rewrite_what_the_author_wrote() {
        // The playground's Format button is `saule fmt`, so the fidelity
        // rules `saule-fmt`'s own corpus test pins apply here too.
        let out = format("local mask: integer = 0xFF\nlocal f = (a, b) => a < b\n");
        assert_eq!(
            out.text.as_deref(),
            Some("local mask: integer = 0xFF\nlocal f = (a, b) => a < b\n")
        );
    }

    /// Completions at the `|` in `src`.
    fn labels_at(src: &str) -> Vec<String> {
        let at = src.find('|').expect("a | marks the caret");
        let source = src.replacen('|', "", 1);
        complete(&source, at).into_iter().map(|c| c.label).collect()
    }

    #[test]
    fn member_completion_offers_the_receivers_members_only() {
        // The playground used to offer every instance method in the library
        // after any `x.`. A string has none — its functions live on `String`.
        let got = labels_at("local s: string = \"hi\"\nlocal n = s.|\n");
        assert!(got.is_empty(), "{got:?}");
        let got = labels_at("local n = String.|\n");
        assert!(got.iter().any(|l| l == "upper"), "{got:?}");
        assert!(!got.iter().any(|l| l == "sqrt"), "{got:?}");
    }

    #[test]
    fn user_class_members_are_completed() {
        let got = labels_at(
            "class Player\n    hp: integer = 10\n    fn heal(n: integer)\n        self.hp = self.hp + n\n    end\nend\nlocal p = Player()\np.|\n",
        );
        assert!(got.iter().any(|l| l == "hp"), "{got:?}");
        assert!(got.iter().any(|l| l == "heal"), "{got:?}");
        assert!(!got.iter().any(|l| l == "upper"), "{got:?}");
    }

    #[test]
    fn bare_words_offer_locals_in_scope() {
        let got = labels_at("local version: float = 1.0\nprintln(ver|)\n");
        assert_eq!(got.first().map(String::as_str), Some("version"), "{got:?}");
    }

    #[test]
    fn a_caret_off_a_char_boundary_answers_nothing_rather_than_panicking() {
        assert!(complete("local s = \"é\"", 12).is_empty());
    }
}
