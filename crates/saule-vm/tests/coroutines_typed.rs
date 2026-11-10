//! The typed coroutine surface: `Coroutine<T>` and `Yielder<T>`.
//!
//! `coroutines.rs` pins the runtime. This pins what the *checker* makes of
//! it: where `T` is learned from (an annotation, a declared return type, the
//! parameter a call fills, the body's own annotation), what a typed handle
//! answers to, `for … in` over a coroutine, and the misuses that are now
//! caught before anything runs.

use saule_lexer::Lexer;
use saule_parser::parse;
use saule_runtime::Value;
use saule_vm::PipelineError;

fn run(src: &str) -> Result<Value, PipelineError> {
    let toks = Lexer::new(src).tokenize().expect("lex");
    let mut module = parse(toks).expect("parse");
    saule_vm::check_and_run(&mut module, "test.sau", src)
}

fn text(src: &str) -> String {
    match run(src) {
        Ok(Value::Str(s)) => (*s).clone(),
        Ok(other) => panic!("expected a string, got {other:?}\n--- source ---\n{src}"),
        Err(e) => panic!("pipeline failed: {e:?}\n--- source ---\n{src}"),
    }
}

/// The program must be rejected before it runs; returns the diagnostic.
fn rejected(src: &str) -> String {
    match run(src) {
        Err(PipelineError::Typeck(e)) => format!("{e}"),
        Err(PipelineError::Semantic(e)) => format!("{e:?}"),
        other => panic!("expected a compile-time rejection, got {other:?}\n--- source ---\n{src}"),
    }
}

/// The program must check, then fail while running; returns the error.
fn fails_at_run_time(src: &str) -> String {
    match run(src) {
        Err(PipelineError::Runtime(e)) => format!("{e}"),
        other => panic!("expected a runtime failure, got {other:?}\n--- source ---\n{src}"),
    }
}

// ── Where `T` comes from ─────────────────────────────────────────────────

#[test]
fn t_comes_from_the_declared_type_of_the_local() {
    // `co` is unannotated: `Yielder<integer>` is learned from the binding.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
    co.yield(2)
  end)
  local a: integer? = gen.resume()
  local b: integer? = gen.resume()
  return tostring(a) .. tostring(b)
end
go()
"#
        ),
        "12"
    );
}

#[test]
fn t_comes_from_a_declared_return_type() {
    // The generator-factory shape.
    assert_eq!(
        text(
            r#"
fn countdown(n: integer) -> Coroutine<integer>
  return Coroutine.create(fn(co)
    for i = n, 1, -1 do
      co.yield(i)
    end
  end)
end
fn go() -> string
  local log: string = ""
  for v in countdown(3) do
    log = log .. v
  end
  return log
end
go()
"#
        ),
        "321"
    );
}

#[test]
fn t_comes_from_the_parameter_a_call_fills() {
    assert_eq!(
        text(
            r#"
fn total(gen: Coroutine<integer>) -> integer
  local sum: integer = 0
  for v in gen do
    sum = sum + v
  end
  return sum
end
fn go() -> string
  return "" .. total(Coroutine.create(fn(co)
    co.yield(20)
    co.yield(22)
  end))
end
go()
"#
        ),
        "42"
    );
}

#[test]
fn t_comes_from_an_assignment_to_a_typed_variable() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: Coroutine<string>? = nil
  gen = Coroutine.create(fn(co)
    co.yield("assigned")
  end)
  return gen!.resume() ?? "none"
end
go()
"#
        ),
        "assigned"
    );
}

#[test]
fn t_comes_from_the_bodys_own_annotation() {
    // No declared destination at all: the annotated yielder is the only
    // statement of `T`, and it is enough.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen = Coroutine.create(fn(co: Yielder<string>)
    co.yield("annotated")
  end)
  local s: string? = gen.resume()
  return s ?? "none"
end
go()
"#
        ),
        "annotated"
    );
}

#[test]
fn a_yielder_can_be_passed_down_with_its_type() {
    assert_eq!(
        text(
            r#"
fn emitTwice(co: Yielder<integer>, n: integer)
  co.yield(n)
  co.yield(n)
end
fn go() -> string
  local gen: Coroutine<integer> = Coroutine.create(fn(co)
    emitTwice(co, 7)
  end)
  local log: string = ""
  for v in gen do
    log = log .. v
  end
  return log
end
go()
"#
        ),
        "77"
    );
}

#[test]
fn a_coroutine_of_a_class_type_yields_instances() {
    assert_eq!(
        text(
            r#"
class Point
  local x: integer
  fn init(x: integer)
    self.x = x
  end
  fn getX() -> integer
    return self.x
  end
end
fn go() -> string
  local gen: Coroutine<Point> = Coroutine.create(fn(co)
    co.yield(Point(1))
    co.yield(Point(2))
  end)
  local sum: integer = 0
  for p in gen do
    sum = sum + p.getX()
  end
  return "" .. sum
end
go()
"#
        ),
        "3"
    );
}

#[test]
fn coroutines_can_be_kept_in_a_typed_table() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gens: table<Coroutine<integer>> = {}
  for i = 1, 3 do
    gens[i] = Coroutine.create(fn(co)
      co.yield(i * 100)
    end)
  end
  local sum: integer = 0
  for i = 1, #gens do
    sum = sum + (gens[i].resume() ?? 0)
  end
  return "" .. sum
end
go()
"#
        ),
        "600"
    );
}

// ── What a typed handle answers ──────────────────────────────────────────

#[test]
fn every_method_has_its_declared_type() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
  end)
  local v: integer? = gen.resume()
  local s: string = gen.status()
  local done: boolean = gen.isDone()
  local err: string? = gen.error()
  local next: fn() -> integer? = gen.iter()
  return tostring(v) .. s .. tostring(done) .. tostring(err) .. tostring(next())
end
go()
"#
        ),
        "1suspendedfalsenilnil"
    );
}

#[test]
fn a_coroutine_of_any_yields_anything() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: Coroutine<any> = Coroutine.create(fn(co)
    co.yield(1)
    co.yield("two")
    co.yield(3.5)
  end)
  local log: string = ""
  for v in gen do
    log = log .. tostring(v) .. ","
  end
  return log
end
go()
"#
        ),
        "1,two,3.5,"
    );
}

#[test]
fn a_nullable_element_type_is_allowed() {
    // `nil` ends a `for … in`, so a `Coroutine<integer?>` is driven by hand.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: Coroutine<integer?> = Coroutine.create(fn(co)
    co.yield(nil)
    co.yield(5)
  end)
  local a: integer? = gen.resume()
  local b: integer? = gen.resume()
  return tostring(a) .. "," .. tostring(b) .. "," .. gen.status()
end
go()
"#
        ),
        "nil,5,suspended"
    );
}

// ── `for … in` ───────────────────────────────────────────────────────────

#[test]
fn for_in_binds_the_element_type() {
    // `v` is an `integer`: integer arithmetic on it is accepted, and an
    // integer-only slot takes it without a cast.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: Coroutine<integer> = Coroutine.create(fn(co)
    for i = 1, 4 do
      co.yield(i)
    end
  end)
  local product: integer = 1
  for v in gen do
    local n: integer = v
    product = product * n
  end
  return "" .. product
end
go()
"#
        ),
        "24"
    );
}

#[test]
fn for_in_accepts_a_matching_annotation() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: Coroutine<string> = Coroutine.create(fn(co)
    co.yield("a")
    co.yield("b")
  end)
  local log: string = ""
  for s: string in gen do
    log = log .. s
  end
  return log
end
go()
"#
        ),
        "ab"
    );
}

#[test]
fn breaking_out_of_for_in_leaves_the_coroutine_suspended() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: Coroutine<integer> = Coroutine.create(fn(co)
    for i = 1, 10 do
      co.yield(i)
    end
  end)
  for v in gen do
    if v == 3 then
      break
    end
  end
  local after: integer? = gen.resume()
  return gen.status() .. tostring(after)
end
go()
"#
        ),
        "suspended4"
    );
}

#[test]
fn for_in_over_a_finished_coroutine_runs_no_iterations() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
  end)
  for v in gen do
  end
  local count: integer = 0
  for v in gen do
    count = count + 1
  end
  return gen.status() .. count
end
go()
"#
        ),
        "dead0"
    );
}

#[test]
fn for_in_raises_when_the_coroutine_fails_part_way() {
    // An explicit `resume` reports a failure; a loop has nowhere to report
    // it, and ending quietly would look like the generator had finished.
    let e = fails_at_run_time(
        r#"
fn go() -> string
  local gen: Coroutine<integer> = Coroutine.create(fn(co)
    co.yield(1)
    throw "mid-loop"
  end)
  local seen: integer = 0
  for v in gen do
    seen = seen + v
  end
  return "unreachable"
end
go()
"#,
    );
    assert!(e.contains("coroutine failed during `for … in`"), "got: {e}");
    assert!(e.contains("mid-loop"), "got: {e}");
}

// ── Caught before it runs ────────────────────────────────────────────────

#[test]
fn yielding_the_wrong_type_is_rejected() {
    let e = rejected(
        r#"
local gen: Coroutine<integer> = Coroutine.create(fn(co)
  co.yield("one")
end)
1
"#,
    );
    assert!(e.contains("Yielder.yield") && e.contains("integer"), "got: {e}");
}

#[test]
fn yielding_the_wrong_type_through_a_passed_down_yielder_is_rejected() {
    let e = rejected(
        r#"
fn helper(co: Yielder<integer>)
  co.yield(true)
end
1
"#,
    );
    assert!(e.contains("Yielder.yield"), "got: {e}");
}

#[test]
fn a_body_annotated_with_another_type_is_rejected() {
    let e = rejected(
        r#"
local gen: Coroutine<integer> = Coroutine.create(fn(co: Yielder<string>)
  co.yield("x")
end)
1
"#,
    );
    assert!(
        e.contains("Coroutine<string>") && e.contains("Coroutine<integer>"),
        "got: {e}"
    );
}

#[test]
fn resume_answers_the_element_type_and_nothing_else() {
    let e = rejected(
        r#"
local gen: Coroutine<integer> = Coroutine.create(fn(co)
  co.yield(1)
end)
local s: string? = gen.resume()
1
"#,
    );
    assert!(e.contains("integer?") && e.contains("string?"), "got: {e}");
    // Nullable: it is `nil` once the coroutine is done.
    let e = rejected(
        r#"
local gen: Coroutine<integer> = Coroutine.create(fn(co)
  co.yield(1)
end)
local n: integer = gen.resume()
1
"#,
    );
    assert!(e.contains("integer"), "got: {e}");
}

#[test]
fn a_typed_resume_takes_no_arguments() {
    let e = rejected(
        r#"
local gen: Coroutine<integer> = Coroutine.create(fn(co)
  co.yield(1)
end)
gen.resume(5)
1
"#,
    );
    assert!(e.contains("Coroutine.resume"), "got: {e}");
}

#[test]
fn each_handle_has_only_its_own_methods() {
    let e = rejected(
        r#"
local gen: Coroutine<integer> = Coroutine.create(fn(co)
  co.yield(1)
end)
gen.yield(1)
1
"#,
    );
    assert!(e.contains("yield") && e.contains("Coroutine"), "got: {e}");
    let e = rejected(
        r#"
local gen: Coroutine<integer> = Coroutine.create(fn(co)
  co.resume()
end)
1
"#,
    );
    assert!(e.contains("resume") && e.contains("Yielder"), "got: {e}");
    let e = rejected(
        r#"
local gen: Coroutine<integer> = Coroutine.create(fn(co)
  co.yield(1)
end)
gen.restart()
1
"#,
    );
    assert!(e.contains("restart"), "got: {e}");
}

#[test]
fn the_body_must_be_a_function_taking_a_yielder() {
    let e = rejected("local gen: Coroutine<integer> = Coroutine.create(42)\n1");
    assert!(e.contains("Coroutine.create"), "got: {e}");
    let e = rejected("local gen: Coroutine<integer> = Coroutine.create()\n1");
    assert!(e.contains("Coroutine.create"), "got: {e}");
}

#[test]
fn a_for_binding_annotated_with_another_type_is_rejected() {
    let e = rejected(
        r#"
local gen: Coroutine<integer> = Coroutine.create(fn(co)
  co.yield(1)
end)
for s: string in gen do
end
1
"#,
    );
    assert!(e.contains("`s`") && e.contains("integer"), "got: {e}");
}

#[test]
fn a_coroutine_cannot_be_passed_where_another_element_type_is_wanted() {
    let e = rejected(
        r#"
fn total(gen: Coroutine<integer>) -> integer
  return 0
end
local names: Coroutine<string> = Coroutine.create(fn(co)
  co.yield("a")
end)
total(names)
1
"#,
    );
    assert!(e.contains("Coroutine<integer>"), "got: {e}");
}

// ── The dynamic escape hatch ─────────────────────────────────────────────

#[test]
fn a_body_held_as_any_gets_the_whole_lua_protocol() {
    // What the typed contract leaves out — arguments to the first resume,
    // values in through `resume(x)`, a return value — is still there for a
    // body the checker was told nothing about.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local body: any = fn(co: any, start: integer) -> string
    local got: any = co.yield(start)
    return "got " .. tostring(got)
  end
  local gen: any = Coroutine.create(body)
  local first: any = gen.resume(10)
  local last: any = gen.resume("back")
  return tostring(first) .. "/" .. tostring(last)
end
go()
"#
        ),
        "10/got back"
    );
}
