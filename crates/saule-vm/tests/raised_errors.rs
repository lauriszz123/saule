//! `error(msg, level)` — what it says, where it points, and what the stack
//! trace reports with it.
//!
//! The fixtures under `tests/errors/exceptions/error_*` only prove these
//! programs fail. What each failure *says* is the behaviour, so it is pinned
//! here: the message, the line the caret is on, the caret's label, and the
//! line every frame of the trace is reported at. Each positive case has a
//! negative beside it — the same code where the error must not happen, or a
//! different failure that must not be mistaken for one.

use saule_lexer::Lexer;
use saule_parser::parse;
use saule_runtime::RuntimeError;
use saule_vm::{PipelineError, TraceFrame};

/// Run `src`, which must fail at runtime, and return the error with the trace
/// recorded for it.
fn fail(src: &str) -> (RuntimeError, Vec<TraceFrame>) {
    saule_vm::take_stack_trace();
    let toks = Lexer::new(src).tokenize().expect("lex");
    let mut module = parse(toks).expect("parse");
    match saule_vm::check_and_run(&mut module, "test.sau", src) {
        Err(PipelineError::Runtime(e)) => (e, saule_vm::take_stack_trace().frames),
        Err(other) => panic!("expected a runtime error, got: {other:?}"),
        Ok(v) => panic!("expected a runtime error, got the value {v:?}"),
    }
}

/// Run `src`, which must succeed.
fn succeed(src: &str) {
    let toks = Lexer::new(src).tokenize().expect("lex");
    let mut module = parse(toks).expect("parse");
    if let Err(e) = saule_vm::check_and_run(&mut module, "test.sau", src) {
        panic!("expected success, got: {e:?}");
    }
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset].matches('\n').count() + 1
}

/// What a `Raised` error reads as: message, the caret's line, its label.
fn raised(src: &str, e: &RuntimeError) -> (String, usize, String) {
    match e {
        RuntimeError::Raised {
            message,
            span,
            label,
            ..
        } => (message.clone(), line_of(src, span.start), label.clone()),
        other => panic!("expected `Raised`, got: {other:?}"),
    }
}

fn trace_lines(frames: &[TraceFrame]) -> Vec<(String, Option<usize>)> {
    frames
        .iter()
        .map(|f| (f.function.clone(), f.line))
        .collect()
}

/// `check` raises at `LEVEL`; `caller` calls it on line 8; the top level
/// calls `caller` on line 11.
fn three_deep(level: &str) -> String {
    format!(
        r#"fn check(n: integer)
  if n < 0 then
    error("n must be non-negative, got " .. n{level})
  end
end

fn caller()
  check(-1)
end

caller()
"#
    )
}

fn expected_trace() -> Vec<(String, Option<usize>)> {
    vec![
        ("check".to_string(), Some(3)),
        ("caller".to_string(), Some(8)),
        ("<top level>".to_string(), Some(11)),
    ]
}

// ── the message is the program's own ────────────────────────────────────────

#[test]
fn an_error_is_shown_as_written_with_no_category_in_front() {
    let src = three_deep("");
    let (e, _) = fail(&src);
    // Not "type error: …": nothing went wrong with the language here.
    assert_eq!(e.to_string(), "n must be non-negative, got -1");
}

// ── level 1: the line that called `error` ──────────────────────────────────

#[test]
fn level_one_is_the_default_and_points_at_the_error_call() {
    for level in ["", ", 1"] {
        let src = three_deep(level);
        let (e, frames) = fail(&src);
        assert_eq!(
            raised(&src, &e),
            (
                "n must be non-negative, got -1".to_string(),
                3,
                "raised here".to_string()
            ),
            "level `{level}`"
        );
        assert_eq!(trace_lines(&frames), expected_trace(), "level `{level}`");
    }
}

// ── level 2 and beyond: the caller's line ──────────────────────────────────

#[test]
fn level_two_points_at_the_call_into_the_raising_function() {
    let src = three_deep(", 2");
    let (e, frames) = fail(&src);
    assert_eq!(
        raised(&src, &e),
        (
            "n must be non-negative, got -1".to_string(),
            8,
            "raised at level 2: blamed on this call".to_string()
        )
    );
    // The trace is unchanged by the level: `check` is still where `error`
    // was called, not the line the caret was moved to.
    assert_eq!(trace_lines(&frames), expected_trace());
}

#[test]
fn level_three_points_one_further_out() {
    let src = three_deep(", 3");
    let (e, frames) = fail(&src);
    let (_, line, label) = raised(&src, &e);
    assert_eq!(line, 11);
    assert_eq!(label, "raised at level 3: blamed on this call");
    assert_eq!(trace_lines(&frames), expected_trace());
}

#[test]
fn a_level_deeper_than_the_stack_blames_the_outermost_call() {
    let src = three_deep(", 9");
    let (e, _) = fail(&src);
    let (_, line, _) = raised(&src, &e);
    assert_eq!(line, 11);
}

#[test]
fn level_two_at_the_top_level_has_no_caller_and_stays_on_the_error_call() {
    let src = "local x: integer = 1\nerror(\"top\", 2)\n";
    let (e, _) = fail(src);
    assert_eq!(
        raised(src, &e),
        ("top".to_string(), 2, "raised here".to_string())
    );
}

#[test]
fn a_level_below_one_is_refused() {
    for level in ["0", "-1"] {
        let src = three_deep(&format!(", {level}"));
        let (e, _) = fail(&src);
        assert!(
            !matches!(e, RuntimeError::Raised { .. }),
            "level {level} raised anyway: {e:?}"
        );
        assert!(
            e.to_string().contains("error level must be 1 or more"),
            "level {level}: {e}"
        );
    }
}

// ── where it must not happen ────────────────────────────────────────────────

#[test]
fn a_guard_that_does_not_fire_raises_nothing() {
    succeed(&three_deep(", 2").replace("check(-1)", "check(1)"));
}

#[test]
fn error_cannot_be_caught() {
    // Documented in DOCS.md: `throw` is the catchable one.
    let src = "try\n  error(\"boom\")\ncatch e: string\n  println(e)\nend\n";
    let (e, _) = fail(src);
    assert_eq!(raised(src, &e).0, "boom");
}

#[test]
fn another_natives_failure_is_not_reported_as_raised() {
    let (e, _) = fail(NATIVE_FAILURE);
    assert!(
        !matches!(e, RuntimeError::Raised { .. }),
        "a native's own failure became `Raised`: {e:?}"
    );
}

/// A native refusing its argument at runtime — the checker can't see a
/// negative limit coming.
const NATIVE_FAILURE: &str =
    "local out: string, n: integer = String.gsub(\"aaa\", \"a\", \"b\", -1)\n";

#[test]
fn a_level_does_not_leak_into_the_next_failure() {
    // A coroutine's failure is caught by `resume`. If the level `error` left
    // behind survived that, the next unrelated failure would be blamed on a
    // caller it has nothing to do with.
    let src = r#"fn go()
  local gen: any = Coroutine.create(fn(co: any) -> nil
    error("inside", 2)
  end)
  gen.resume()
end
go()
"#;
    let src = format!("{src}{NATIVE_FAILURE}");
    let (e, _) = fail(&src);
    assert!(
        !matches!(e, RuntimeError::Raised { .. }),
        "the coroutine's level leaked: {e:?}"
    );
}

// ── the running frame's line ────────────────────────────────────────────────

#[test]
fn the_innermost_frame_of_any_runtime_error_has_a_line() {
    let src = r#"fn divide(a: integer, b: integer) -> integer
  return a / b
end
fn run() -> integer
  return divide(1, 0)
end
run()
"#;
    let (e, frames) = fail(src);
    assert!(matches!(e, RuntimeError::DivisionByZero { .. }), "{e:?}");
    assert_eq!(frames.first().map(|f| f.line), Some(Some(2)));
}
