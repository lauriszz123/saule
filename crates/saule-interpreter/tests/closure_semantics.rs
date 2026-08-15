//! Closure behaviour under the exact capture set (Phase 0.6 part B).
//!
//! `VM_DESIGN.md` §23.2 singles this area out: per-iteration capture,
//! self-recursive locals and upvalue lifetime are where a subtle divergence
//! is most likely and least likely to be caught by a fixture that only
//! checks the exit code. So these assert **values**.
//!
//! What changed underneath them: a lambda used to capture every identifier
//! its body mentioned, and to fall back to capturing its whole defining
//! scope whenever the body contained a nested declaration. It now captures
//! exactly the bindings `saule-semantic` proved it refers to. Every case
//! below is one where getting that set wrong produces a wrong answer rather
//! than a crash.

use saule_interpreter::{Value, check_and_run};
use saule_lexer::Lexer;
use saule_parser::parse;

fn eval(src: &str) -> Value {
    saule_interpreter::init();
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    match check_and_run(&module) {
        Ok(v) => v,
        Err(e) => panic!("pipeline failed: {e:?}\n--- source ---\n{src}"),
    }
}

fn int(src: &str) -> i64 {
    match eval(src) {
        Value::Int(n) => n,
        other => panic!("expected an integer, got {other:?}"),
    }
}

fn text(src: &str) -> String {
    match eval(src) {
        Value::Str(s) => (*s).clone(),
        other => panic!("expected a string, got {other:?}"),
    }
}

#[test]
fn a_closure_sees_writes_made_after_it_was_built() {
    // Live binding, not a snapshot. Capturing by value would answer 1.
    assert_eq!(
        int(r#"
fn outer() -> integer
  local n: integer = 1
  local read = fn() -> integer
    return n
  end
  n = 41
  return read() + 1
end
outer()
"#),
        42
    );
}

#[test]
fn a_closure_writes_through_to_the_enclosing_binding() {
    // The other direction: the enclosing scope must observe the closure's
    // writes. An assignment target is a reference, so it has to be part of
    // the capture set too.
    assert_eq!(
        int(r#"
fn outer() -> integer
  local n: integer = 0
  local bump = fn() -> nil
    n = n + 1
  end
  bump()
  bump()
  bump()
  return n
end
outer()
"#),
        3
    );
}

#[test]
fn a_captured_binding_outlives_the_scope_that_declared_it() {
    // The counter-factory shape from `capture_flat`'s own documentation:
    // the closure is returned, so `i` must survive `counter` returning.
    assert_eq!(
        int(r#"
fn counter() -> fn() -> integer
  local i: integer = 0
  return fn() -> integer
    i = i + 1
    return i
  end
end
local next = counter()
next()
next()
next()
"#),
        3
    );
}

#[test]
fn two_closures_over_one_binding_share_it() {
    assert_eq!(
        int(r#"
fn pair() -> integer
  local n: integer = 0
  local inc = fn() -> nil
    n = n + 10
  end
  local dec = fn() -> nil
    n = n - 1
  end
  inc()
  inc()
  dec()
  return n
end
pair()
"#),
        19
    );
}

#[test]
fn two_calls_produce_independent_bindings() {
    // Each activation gets its own `i`; sharing one would answer 2.
    assert_eq!(
        int(r#"
fn counter() -> fn() -> integer
  local i: integer = 0
  return fn() -> integer
    i = i + 1
    return i
  end
end
local a = counter()
local b = counter()
a()
b()
"#),
        1
    );
}

#[test]
fn a_self_recursive_local_closure_still_recurses() {
    // `FunctionObject::self_name` / `Environment::drop_capture` territory:
    // the name is captured like any other, and the cycle it would close is
    // broken by re-binding it per call.
    assert_eq!(
        int(r#"
fn run() -> integer
  local fact = fn(n: integer) -> integer
    if n <= 1 then
      return 1
    end
    return n * fact(n - 1)
  end
  return fact(5)
end
run()
"#),
        120
    );
}

#[test]
fn mutual_reference_through_two_levels_of_nesting() {
    // The inner lambda reaches a binding two function boundaries away, so
    // the middle one has to carry it even though it never mentions it.
    assert_eq!(
        int(r#"
fn outer() -> integer
  local base: integer = 40
  local mid = fn() -> fn() -> integer
    return fn() -> integer
      return base + 2
    end
  end
  local inner = mid()
  return inner()
end
outer()
"#),
        42
    );
}

#[test]
fn a_parameter_shadows_a_captured_name() {
    // The old analysis over-approximated and relied on lookup order to make
    // shadowing win. The exact set must not capture `x` at all here — and
    // either way the answer is the parameter's.
    assert_eq!(
        int(r#"
fn outer() -> integer
  local x: integer = 1
  local f = fn(x: integer) -> integer
    return x
  end
  return f(99)
end
outer()
"#),
        99
    );
}

#[test]
fn a_nested_declaration_no_longer_forces_whole_scope_capture() {
    // The exact case the old analysis gave up on: a `Stmt::Decl` inside a
    // lambda body set `opaque` and fell back to capturing the entire
    // enclosing scope. Behaviour must be unchanged now that it does not.
    assert_eq!(
        int(r#"
fn outer() -> integer
  local wanted: integer = 7
  local f = fn() -> integer
    fn helper(n: integer) -> integer
      return n
    end
    return wanted * 6
  end
  return f()
end
outer()
"#),
        42
    );
}

#[test]
fn the_whole_scope_fallback_is_gone() {
    // The leak itself, asserted rather than inferred.
    //
    // The body contains a nested declaration, which is exactly what used to
    // set `opaque` and make the lambda capture its entire defining scope —
    // pinning `big` alive for as long as the closure lived, despite the body
    // never mentioning it. Inspecting the closure's own environment is the
    // only way to see the difference: both versions compute 7.
    let f = match eval(
        r#"
fn outer() -> fn() -> integer
  local wanted: integer = 7
  local big: table<integer> = {1, 2, 3}
  return fn() -> integer
    fn helper(n: integer) -> integer
      return n
    end
    return wanted
  end
end
outer()
"#,
    ) {
        Value::Function(f) => f,
        other => panic!("expected a function, got {other:?}"),
    };

    let closure = f.closure.borrow();
    assert!(
        closure.get("wanted").is_some(),
        "the closure lost a binding its body needs"
    );
    assert!(
        closure.get("big").is_none(),
        "the closure is still holding a binding it never referenced"
    );
}

#[test]
fn a_lambda_inside_a_method_captures_self() {
    // `self` is not an identifier, so it never appears in the upvalue list.
    // The resolver tracks it separately; if that were dropped, this would
    // fail to resolve `self` at all.
    assert_eq!(
        text(r#"
class Greeter
  fn init(name: string)
    self.name = name
  end
  name: string

  fn greeter() -> fn() -> string
    return fn() -> string
      return "hi " .. self.name
    end
  end
end
local g = Greeter("ada")
g.greeter()()
"#),
        "hi ada"
    );
}

#[test]
fn self_reaches_through_two_nested_lambdas() {
    // Same rule as an ordinary upvalue: the middle closure must hold `self`
    // so the inner one can reach it.
    assert_eq!(
        text(r#"
class Box
  fn init(v: string)
    self.v = v
  end
  v: string

  fn deep() -> fn() -> fn() -> string
    return fn() -> fn() -> string
      return fn() -> string
        return self.v
      end
    end
  end
end
Box("deep").deep()()()
"#),
        "deep"
    );
}

#[test]
fn a_class_static_is_reachable_from_a_lambda_in_a_method() {
    // Statics do not travel through the capture set at all — they hang off
    // the scope's `statics_owner`, which `capture_flat` preserves. Worth
    // pinning, because narrowing the capture set is exactly the change that
    // would have broken it if they did.
    assert_eq!(
        int(r#"
class Counter
  static total: integer = 5

  static fn reader() -> fn() -> integer
    return fn() -> integer
      return total * 2
    end
  end
end
Counter.reader()()
"#),
        10
    );
}

#[test]
fn a_module_level_binding_is_reachable_from_a_closure() {
    // Top-level names resolve through the scope root rather than being
    // captured, so they must stay reachable even though they are absent
    // from every upvalue list.
    assert_eq!(
        int(r#"
local base: integer = 20

fn make() -> fn() -> integer
  return fn() -> integer
    return base + 1
  end
end
make()()
"#),
        21
    );
}

#[test]
fn a_closure_in_a_top_level_block_captures_a_block_local() {
    // A `local` inside a block at top level is an ordinary local of the
    // module body, not a module slot — so a closure beside it genuinely
    // captures it.
    assert_eq!(
        int(r#"
local out: integer = 0
if true then
  local hidden: integer = 9
  local f = fn() -> integer
    return hidden
  end
  out = f()
end
out
"#),
        9
    );
}

#[test]
fn per_iteration_capture_gives_each_closure_its_own_value() {
    // `Environment::recycle` gives a loop body a fresh scope per iteration
    // only when something captured the previous one. Three closures, three
    // distinct values.
    assert_eq!(
        int(r#"
fn build() -> integer
  local fns: table<fn() -> integer> = {}
  for i = 1, 3 do
    Table.insert(fns, fn() -> integer
      return i
    end)
  end
  return fns[1]() * 100 + fns[2]() * 10 + fns[3]()
end
build()
"#),
        123
    );
}

#[test]
fn a_loop_body_local_is_captured_per_iteration() {
    assert_eq!(
        int(r#"
fn build() -> integer
  local fns: table<fn() -> integer> = {}
  for i = 1, 3 do
    local doubled: integer = i * 2
    Table.insert(fns, fn() -> integer
      return doubled
    end)
  end
  return fns[1]() * 100 + fns[2]() * 10 + fns[3]()
end
build()
"#),
        246
    );
}
