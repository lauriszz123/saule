//! Differential testing: every program is run under **both** engines and the
//! results compared (`VM_DESIGN.md` §23.2).
//!
//! This is the highest-value test shape for the project. The tree-walker is
//! the oracle — it is ~13k lines that already work and it defines what the
//! language means — so "the VM agrees with it" is a much stronger statement
//! than any hand-written expectation, and it costs nothing to author.
//!
//! Programs the compiler cannot handle yet are skipped rather than failed:
//! `CompileError::Unsupported` is the designed signal for "fall back to the
//! tree-walker" (§21.3), so treating it as a failure would make every
//! not-yet-written feature look like a bug.

use std::rc::Rc;

use saule_interpreter::{Environment, Value};
use saule_lexer::Lexer;
use saule_parser::parse;

/// Outcome of running one program under one engine.
#[derive(Debug, PartialEq)]
enum Outcome {
    Value(String),
    Error(String),
}

fn describe(v: &Value) -> String {
    // Includes the type name, so `1` and `1.0` — which print differently but
    // could be confused — cannot compare equal by accident.
    format!("{}:{}", v.type_name(), v.to_display_string())
}

fn front_end(src: &str) -> saule_ast::Module {
    saule_interpreter::init();
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    let errs = saule_interpreter::analyze_and_prepare(&module, saule_semantic::ModuleSeed::default());
    assert!(errs.is_empty(), "semantic errors in test source: {errs:?}");
    let terrs = saule_interpreter::typeck::check(&module);
    assert!(terrs.is_empty(), "type errors in test source: {terrs:?}");
    module
}

fn tree_walker(module: &saule_ast::Module) -> Outcome {
    let env = Environment::with_prelude();
    match saule_interpreter::run_in(module, &env) {
        Ok(v) => Outcome::Value(describe(&v)),
        Err(e) => Outcome::Error(e.to_string()),
    }
}

/// `None` when the compiler does not support the program yet.
fn vm(module: &saule_ast::Module, src: &str) -> Option<Outcome> {
    let chunk = match saule_vm::compile(module, "diff.sau", src) {
        Ok(c) => c,
        Err(saule_vm::CompileError::Unsupported { .. }) => return None,
        Err(e) => return Some(Outcome::Error(format!("compile error: {e}"))),
    };
    Some(match saule_vm::run_chunk(Rc::new(chunk)) {
        Ok(vs) => Outcome::Value(
            vs.first()
                .map(describe)
                .unwrap_or_else(|| describe(&Value::Nil)),
        ),
        Err(e) => Outcome::Error(e.to_string()),
    })
}

/// Run under both engines and require agreement. Returns `false` when the
/// program is not compilable yet, so a caller can count coverage.
#[must_use]
fn agree(src: &str) -> bool {
    let module = front_end(src);
    let expected = tree_walker(&module);
    match vm(&module, src) {
        None => false,
        Some(got) => {
            assert_eq!(
                got, expected,
                "engines disagreed\n--- source ---{src}\n--- disassembly ---\n{}",
                saule_vm::compile(&module, "diff.sau", src)
                    .map(|c| saule_vm::disasm::chunk(&c))
                    .unwrap_or_default()
            );
            true
        }
    }
}

/// Assert agreement *and* that the VM actually compiled it — for cases the
/// compiler is expected to handle, so a regression to `Unsupported` fails
/// rather than silently skipping.
fn must_agree(src: &str) {
    if !agree(src) {
        let module = front_end(src);
        let why = saule_vm::compile(&module, "diff.sau", src)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "compiled fine on the retry?".into());
        panic!("the compiler refused a program it should handle:
{src}
  -> {why}");
    }
}

// ── literals and locals ───────────────────────────────────────────────────

#[test]
fn integer_literals_large_and_small() {
    // The boundary that matters: `LOADI` carries a 16-bit signed operand and
    // anything past it must go through the constant pool, not wrap.
    must_agree("1");
    must_agree("-1");
    must_agree("32767");
    must_agree("32768");
    must_agree("2147483647");
    must_agree("9223372036854775807");
}

#[test]
fn other_literals() {
    must_agree("true");
    must_agree("false");
    must_agree("nil");
    must_agree("1.5");
    must_agree("\"hello\"");
}

#[test]
fn module_level_locals() {
    must_agree("local x: integer = 7\nx");
    must_agree("local a: integer = 2\nlocal b: integer = 3\na + b");
    must_agree("local x: integer = 1\nx = 5\nx");
}

// ── arithmetic ────────────────────────────────────────────────────────────

#[test]
fn integer_arithmetic_matches() {
    for src in [
        "1 + 2", "10 - 3", "6 * 7", "20 / 6", "20 % 6", "2 ^ 10",
        "1 + 2 * 3 - 4", "(1 + 2) * 3", "-5 + 1", "~5", "5 & 3", "5 | 3",
        "5 ~ 3", "1 << 4", "256 >> 4",
    ] {
        must_agree(src);
    }
}

#[test]
fn integer_overflow_wraps_the_same_way() {
    // `integer` is i64 and overflow wraps (README, "Integer Overflow"). The
    // VM uses `wrapping_*` for exactly this reason; a mismatch here would be
    // a silent divergence on a documented behaviour.
    must_agree("9223372036854775807 + 1");
    must_agree("9223372036854775807 * 2");
}

#[test]
fn float_arithmetic_matches() {
    for src in [
        "1.5 + 2.5", "1.0 - 2.5", "1.5 * 2.0", "7.0 / 2.0", "7.5 % 2.0", "2.0 ^ 10.0",
        "-1.5",
    ] {
        must_agree(src);
    }
}

#[test]
fn division_by_zero_fails_the_same_way() {
    // Integer division by zero is an error; float division by zero is
    // infinity. Both must match the tree-walker, error text included.
    must_agree("1 / 0");
    must_agree("1 % 0");
    must_agree("1.0 / 0.0");
}

#[test]
fn a_negative_integer_exponent_is_an_error_in_both() {
    must_agree("2 ^ -1");
}

// ── comparison and strings ────────────────────────────────────────────────

#[test]
fn comparisons_match() {
    for src in [
        "1 < 2", "2 < 1", "1 <= 1", "2 > 1", "1 >= 2", "1 == 1", "1 != 1",
        "1.5 < 2.5", "2.5 <= 2.5", "3.5 > 1.0", "1.0 == 1.0", "1.0 != 2.0",
        "true == false", "\"a\" == \"a\"", "\"a\" != \"b\"",
    ] {
        must_agree(src);
    }
}

#[test]
fn not_matches() {
    must_agree("not true");
    must_agree("not false");
    must_agree("not nil");
}

#[test]
fn concatenation_matches() {
    must_agree("\"a\" .. \"b\"");
    must_agree("\"n=\" .. 42");
    must_agree("\"x\" .. 1.5");
    must_agree("\"a\" .. \"b\" .. \"c\"");
}

// ── control flow ──────────────────────────────────────────────────────────

#[test]
fn if_else_matches() {
    must_agree("local x: integer = 0\nif true then x = 1 else x = 2 end\nx");
    must_agree("local x: integer = 0\nif false then x = 1 else x = 2 end\nx");
    must_agree("local x: integer = 5\nif x > 3 then x = 100 end\nx");
    must_agree(
        "local x: integer = 2\nlocal r: integer = 0\n\
         if x == 1 then r = 10 elseif x == 2 then r = 20 else r = 30 end\nr",
    );
}

#[test]
fn while_matches() {
    must_agree("local i: integer = 0\nwhile i < 5 do i = i + 1 end\ni");
    must_agree("local i: integer = 0\nlocal s: integer = 0\nwhile i < 10 do i = i + 1 s = s + i end\ns");
    // Never taken.
    must_agree("local i: integer = 9\nwhile i < 5 do i = i + 1 end\ni");
}

#[test]
fn numeric_for_matches() {
    must_agree("local s: integer = 0\nfor i = 1, 10 do s = s + i end\ns");
    must_agree("local s: integer = 0\nfor i = 1, 10, 2 do s = s + i end\ns");
    must_agree("local s: integer = 0\nfor i = 10, 1, -1 do s = s + i end\ns");
    // A loop whose body never runs.
    must_agree("local s: integer = 0\nfor i = 5, 1 do s = s + i end\ns");
    // Appendix B.1's shape, through the compiler this time.
    must_agree("local total: integer = 0\nfor i = 1, 100 do total = total + i end\ntotal");
}

#[test]
fn a_zero_step_is_an_error_in_both() {
    must_agree("local s: integer = 0\nfor i = 1, 10, 0 do s = s + i end\ns");
}

#[test]
fn nested_loops_match() {
    must_agree(
        "local s: integer = 0\n\
         for i = 1, 10 do\n\
           for j = 1, 10 do\n\
             s = s + i * j\n\
           end\n\
         end\n\
         s",
    );
}

#[test]
fn block_scoping_matches() {
    // Sibling blocks share registers in the compiler; that must not make one
    // block observe the other's value.
    must_agree(
        "local out: integer = 0\n\
         if true then local a: integer = 1 out = out + a end\n\
         if true then local b: integer = 2 out = out + b end\n\
         out",
    );
}

// ── the pieces working together ───────────────────────────────────────────

#[test]
fn a_larger_program_matches() {
    must_agree(
        "local total: integer = 0\n\
         local count: integer = 0\n\
         for i = 1, 50 do\n\
           if i % 3 == 0 then\n\
             total = total + i\n\
             count = count + 1\n\
           elseif i % 5 == 0 then\n\
             total = total + i * 2\n\
           end\n\
         end\n\
         total * 1000 + count",
    );
}

#[test]
fn unsupported_constructs_report_rather_than_miscompile() {
    // The contract that makes `--vm` usable before it is finished: anything
    // codegen cannot do yet is refused by name, never guessed at.
    //
    // An `import` is the construct standing in for that here — repoint this
    // at another unsupported one when imports land, the same way the
    // `unimplemented_opcodes_report_rather_than_panic` canary is repointed.
    // The assertion is about the *shape* of the refusal: it names the
    // construct and carries a span, so the CLI can fall back and say why.
    let src = "import Json from \"json\"
1";
    let module = front_end(src);
    match saule_vm::compile(&module, "x.sau", src) {
        Err(saule_vm::CompileError::Unsupported { thing, span }) => {
            assert_eq!(thing, "an import declaration");
            assert!(span.start < span.end, "the refusal must point somewhere");
        }
        other => panic!("expected a clean Unsupported, got {other:?}"),
    }
}

// ── short-circuiting ──────────────────────────────────────────────────────

#[test]
fn and_or_and_coalesce_match() {
    // Lua semantics: `and`/`or` evaluate to one of their *operands*, not to a
    // boolean, so the result's type matters as much as its truthiness.
    for src in [
        "true and false", "false and true", "true or false", "false or true",
        "1 < 2 and 3 < 4", "1 < 2 or 3 > 4", "2 > 3 and 4 > 5",
        "nil ?? 5", "7 ?? 5",
    ] {
        must_agree(src);
    }
}

#[test]
fn short_circuit_really_short_circuits() {
    // If the right operand were evaluated eagerly, this would divide by zero
    // in one engine and not the other — which is exactly the kind of
    // divergence a value-only comparison would miss.
    must_agree("local d: integer = 0\nlocal ok: boolean = d != 0 and 10 / d > 1\nok");
    must_agree("local d: integer = 0\nlocal ok: boolean = d == 0 or 10 / d > 1\nok");
}

// ── functions and calls ───────────────────────────────────────────────────

#[test]
fn a_function_call_matches() {
    must_agree("fn double(n: integer) -> integer\n  return n * 2\nend\ndouble(21)");
    must_agree("fn add(a: integer, b: integer) -> integer\n  return a + b\nend\nadd(2, 3)");
    must_agree("fn zero() -> integer\n  return 0\nend\nzero()");
}

#[test]
fn a_forward_call_matches() {
    // `a` calls `b` before `b` is declared — the reason proto indices are
    // reserved in a pre-pass.
    must_agree(
        "fn a(n: integer) -> integer\n  return b(n) + 1\nend\n\
         fn b(n: integer) -> integer\n  return n * 10\nend\n\
         a(4)",
    );
}

#[test]
fn recursion_matches() {
    // The Phase 2 milestone: `fib` through `CALLK`, compiled from source.
    must_agree(
        "fn fib(n: integer) -> integer\n\
         \x20 if n < 2 then return n end\n\
         \x20 return fib(n - 1) + fib(n - 2)\n\
         end\n\
         fib(20)",
    );
    must_agree(
        "fn fact(n: integer) -> integer\n\
         \x20 if n <= 1 then return 1 end\n\
         \x20 return n * fact(n - 1)\n\
         end\n\
         fact(10)",
    );
}

#[test]
fn a_function_falling_off_the_end_returns_nil_in_both() {
    must_agree("fn nothing(n: integer) -> nil\n  local x: integer = n\nend\nnothing(1)");
}

#[test]
fn early_return_matches() {
    must_agree(
        "fn classify(n: integer) -> integer\n\
         \x20 if n < 0 then return -1 end\n\
         \x20 if n == 0 then return 0 end\n\
         \x20 return 1\n\
         end\n\
         classify(-5) * 100 + classify(0) * 10 + classify(9)",
    );
}

#[test]
fn a_function_used_as_a_value_matches() {
    // Declaring a `fn` also binds its name, so it is not only a `CALLK`
    // target — the tree-walker treats it as a value and so must the VM.
    must_agree("fn f() -> integer\n  return 1\nend\nlocal g = f\n2");
}

// ── break and continue ────────────────────────────────────────────────────

#[test]
fn break_matches() {
    must_agree("local s: integer = 0\nfor i = 1, 100 do\n  if i > 5 then break end\n  s = s + i\nend\ns");
    must_agree("local i: integer = 0\nwhile true do\n  i = i + 1\n  if i >= 7 then break end\nend\ni");
}

#[test]
fn continue_matches() {
    // The trap: `continue` in a numeric `for` must still step the counter.
    // Targeting the body top instead of `FORLOOP` would loop forever.
    must_agree(
        "local s: integer = 0\nfor i = 1, 10 do\n  if i % 2 == 0 then continue end\n  s = s + i\nend\ns",
    );
    must_agree(
        "local i: integer = 0\nlocal s: integer = 0\n\
         while i < 10 do\n  i = i + 1\n  if i % 3 == 0 then continue end\n  s = s + i\nend\ns",
    );
}

#[test]
fn break_leaves_only_the_inner_loop() {
    must_agree(
        "local s: integer = 0\n\
         for i = 1, 5 do\n\
           for j = 1, 5 do\n\
             if j > i then break end\n\
             s = s + 1\n\
           end\n\
         end\n\
         s",
    );
}

// ── everything together ───────────────────────────────────────────────────

#[test]
fn a_program_with_functions_and_loops_matches() {
    must_agree(
        "fn isPrime(n: integer) -> boolean\n\
         \x20 if n < 2 then return false end\n\
         \x20 local i: integer = 2\n\
         \x20 while i * i <= n do\n\
         \x20   if n % i == 0 then return false end\n\
         \x20   i = i + 1\n\
         \x20 end\n\
         \x20 return true\n\
         end\n\
         local count: integer = 0\n\
         local sum: integer = 0\n\
         for n = 1, 200 do\n\
           if isPrime(n) then\n\
             count = count + 1\n\
             sum = sum + n\n\
           end\n\
         end\n\
         sum * 1000 + count",
    );
}

// ── repeat, compound assignment, tables ───────────────────────────────────

#[test]
fn repeat_matches() {
    must_agree("local i: integer = 0\nrepeat i = i + 1 until i >= 5\ni");
    // Always runs once, even when the condition is already true.
    must_agree("local i: integer = 99\nrepeat i = i + 1 until i >= 5\ni");
    // `until` sees a local the body declared — the reason the condition is
    // compiled inside the body's scope.
    must_agree("local n: integer = 0\nrepeat\n  local step: integer = 2\n  n = n + step\nuntil n >= 10\nn");
}

#[test]
fn break_and_continue_work_in_repeat() {
    must_agree("local i: integer = 0\nrepeat\n  i = i + 1\n  if i > 3 then break end\nuntil false\ni");
}

#[test]
fn compound_assignment_matches() {
    for src in [
        "local x: integer = 10\nx += 5\nx",
        "local x: integer = 10\nx -= 3\nx",
        "local x: integer = 10\nx *= 3\nx",
        "local x: integer = 10\nx /= 3\nx",
        "local x: integer = 10\nx %= 3\nx",
        "local s: string = \"a\"\ns ..= \"b\"\ns",
    ] {
        must_agree(src);
    }
}

#[test]
fn table_literals_and_indexing_match() {
    must_agree("local t: table<integer> = {1, 2, 3}\nt[2]");
    must_agree("local t: table<integer> = {}\n#t");
    must_agree("local t: table<integer> = {10, 20, 30}\n#t");
    must_agree("local t: table<integer> = {1, 2, 3}\nt[1] + t[2] + t[3]");
}

#[test]
fn a_table_built_in_a_loop_matches() {
    must_agree(
        "local t: table<integer> = {}\n\
         for i = 1, 5 do\n\
           t[i] = i * i\n\
         end\n\
         t[1] + t[2] + t[3] + t[4] + t[5]",
    );
}

// ── lambdas and closures ──────────────────────────────────────────────────

#[test]
fn a_lambda_matches() {
    must_agree("local f = fn(n: integer) -> integer\n  return n * 2\nend\nf(21)");
    must_agree("local f = (n: integer) => n + 1\nf(41)");
}

#[test]
fn a_closure_reads_its_captured_variable() {
    must_agree(
        "fn make() -> integer\n\
         \x20 local base: integer = 40\n\
         \x20 local f = fn() -> integer\n\
         \x20   return base + 2\n\
         \x20 end\n\
         \x20 return f()\n\
         end\n\
         make()",
    );
}

#[test]
fn a_closure_writes_through_to_its_captured_variable() {
    // The live-binding half: `SETUPVAL`, not a copy.
    must_agree(
        "fn run() -> integer\n\
         \x20 local n: integer = 0\n\
         \x20 local bump = fn() -> nil\n\
         \x20   n = n + 1\n\
         \x20 end\n\
         \x20 bump()\n\
         \x20 bump()\n\
         \x20 bump()\n\
         \x20 return n\n\
         end\n\
         run()",
    );
}

#[test]
fn a_closure_sees_writes_made_after_it_was_built() {
    must_agree(
        "fn run() -> integer\n\
         \x20 local n: integer = 1\n\
         \x20 local read = fn() -> integer\n\
         \x20   return n\n\
         \x20 end\n\
         \x20 n = 41\n\
         \x20 return read() + 1\n\
         end\n\
         run()",
    );
}

#[test]
fn capture_threads_through_two_function_boundaries() {
    // The middle closure must gain an upvalue it never mentions, so the
    // inner one reaches through it rather than past it.
    must_agree(
        "fn outer() -> integer\n\
         \x20 local base: integer = 40\n\
         \x20 local mid = fn() -> integer\n\
         \x20   local inner = fn() -> integer\n\
         \x20     return base + 2\n\
         \x20   end\n\
         \x20   return inner()\n\
         \x20 end\n\
         \x20 return mid()\n\
         end\n\
         outer()",
    );
}

#[test]
fn a_lambda_capturing_nothing_captures_nothing() {
    must_agree("local f = fn() -> integer\n  return 7\nend\nf()");
}

#[test]
fn two_closures_share_one_captured_binding() {
    must_agree(
        "fn pair() -> integer\n\
         \x20 local n: integer = 0\n\
         \x20 local inc = fn() -> nil\n\
         \x20   n = n + 10\n\
         \x20 end\n\
         \x20 local dec = fn() -> nil\n\
         \x20   n = n - 1\n\
         \x20 end\n\
         \x20 inc()\n\
         \x20 inc()\n\
         \x20 dec()\n\
         \x20 return n\n\
         end\n\
         pair()",
    );
}

// ── classes ───────────────────────────────────────────────────────────────

#[test]
fn a_static_method_call_matches() {
    // The shape every file in `benchmarks/sau/` uses.
    must_agree("class C\n  static fn twice(n: integer) -> integer\n    return n * 2\n  end\nend\nC.twice(21)");
}

#[test]
fn construction_and_field_reads_match() {
    must_agree(
        "class P\n\
         \x20 fn init(h: integer)\n\
         \x20   self.health = h\n\
         \x20 end\n\
         \x20 health: integer\n\
         end\n\
         local p = P(42)\n\
         p.health",
    );
}

#[test]
fn field_defaults_are_applied() {
    must_agree(
        "class P\n\
         \x20 fn init()\n\
         \x20   self.name = \"x\"\n\
         \x20 end\n\
         \x20 health: integer = 100\n\
         \x20 name: string\n\
         end\n\
         local p = P()\n\
         p.health",
    );
}

#[test]
fn instance_methods_match() {
    must_agree(
        "class Counter\n\
         \x20 fn init()\n\
         \x20   self.n = 0\n\
         \x20 end\n\
         \x20 n: integer\n\
         \x20 fn bump(by: integer) -> nil\n\
         \x20   self.n = self.n + by\n\
         \x20 end\n\
         \x20 fn value() -> integer\n\
         \x20   return self.n\n\
         \x20 end\n\
         end\n\
         local c = Counter()\n\
         c.bump(5)\n\
         c.bump(7)\n\
         c.value()",
    );
}

#[test]
fn a_static_field_matches() {
    must_agree(
        "class Reg\n  static total: integer = 5\n\
         \x20 static fn add(n: integer) -> integer\n\
         \x20   Reg.total = Reg.total + n\n\
         \x20   return Reg.total\n\
         \x20 end\n\
         end\n\
         Reg.add(3)",
    );
}

#[test]
fn inheritance_and_overrides_dispatch_dynamically() {
    // The prefix invariant in action: `describe` is resolved to a vtable
    // slot against the static type, and a subclass receiver reaches the
    // override through that same slot.
    must_agree(
        "class Base\n\
         \x20 fn init()\n\
         \x20   self.a = 1\n\
         \x20 end\n\
         \x20 a: integer\n\
         \x20 fn describe() -> integer\n\
         \x20   return 10\n\
         \x20 end\n\
         end\n\
         class Child extends Base\n\
         \x20 fn init()\n\
         \x20   self.super()\n\
         \x20   self.b = 2\n\
         \x20 end\n\
         \x20 b: integer\n\
         \x20 fn describe() -> integer\n\
         \x20   return 20\n\
         \x20 end\n\
         end\n\
         local c = Child()\n\
         c.describe() + c.a",
    );
}

// ── enums and `match` ─────────────────────────────────────────────────────

const ENUM: &str = "enum Status
  Ok
  Warn
  Failed
end
";

#[test]
fn a_bare_variant_is_a_stable_singleton() {
    must_agree(&format!("{ENUM}local a = Status.Ok
local b = Status.Ok
a == b"));
    must_agree(&format!("{ENUM}Status.Ok == Status.Failed"));
}

#[test]
fn a_valued_variant_carries_its_value() {
    must_agree("enum E
  A = \"alpha\"
  B = \"beta\"
end
E.A.value");
}

#[test]
fn a_switchable_match_matches() {
    // Every arm a distinct variant of one enum — the `GETTAG` + `SWITCH`
    // shape, O(1) instead of O(arms).
    for v in ["Ok", "Warn", "Failed"] {
        must_agree(&format!(
            "{ENUM}local s = Status.{v}
match s
  case Status.Ok then 1
               case Status.Warn then 2
  case Status.Failed then 3
end"
        ));
    }
}

#[test]
fn a_match_with_a_wildcard_default_matches() {
    for v in ["Ok", "Failed"] {
        must_agree(&format!(
            "{ENUM}local s = Status.{v}
match s
  case Status.Ok then 1
  case _ then 99
end"
        ));
    }
}

#[test]
fn a_guarded_match_falls_back_to_a_chain_and_still_matches() {
    // A guard makes the switch inapplicable; the chain form must agree.
    must_agree(&format!(
        "{ENUM}local n: integer = 5
local s = Status.Ok
         match s
  case Status.Ok when n > 3 then 10
  case Status.Ok then 20
           case _ then 30
end"
    ));
    must_agree(&format!(
        "{ENUM}local n: integer = 1
local s = Status.Ok
         match s
  case Status.Ok when n > 3 then 10
  case Status.Ok then 20
           case _ then 30
end"
    ));
}

#[test]
fn a_match_over_literals_matches() {
    for n in [1, 2, 7] {
        must_agree(&format!(
            "local n: integer = {n}
match n
  case 1 then 10
  case 2 then 20
  case _ then 99
end"
        ));
    }
}

#[test]
fn a_tuple_variant_carries_its_payload() {
    must_agree(
        "enum Event
  Quit
  Click(x: integer, y: integer)
end
         local e = Event.Click(3, 4)
         match e
  case Event.Click(x, y) then x * 10 + y
  case _ then 0
end",
    );
}

#[test]
fn a_match_binding_the_scrutinee_matches() {
    must_agree(&format!(
        "{ENUM}local s = Status.Warn
match s
  case Status.Ok then 1
  case other then 2
end"
    ));
}

// ── interfaces ────────────────────────────────────────────────────────────

const SHAPES: &str = "interface Shape\n  fn area() -> integer\n  fn name() -> string\nend\n\
class Square implements Shape\n\
\x20 fn init(s: integer)\n    self.side = s\n  end\n  side: integer\n\
\x20 fn area() -> integer\n    return self.side * self.side\n  end\n\
\x20 fn name() -> string\n    return \"square\"\n  end\n\
end\n\
class Rect implements Shape\n\
\x20 fn init(w: integer, h: integer)\n    self.w = w\n    self.h = h\n  end\n\
\x20 w: integer\n  h: integer\n\
\x20 fn area() -> integer\n    return self.w * self.h\n  end\n\
\x20 fn name() -> string\n    return \"rect\"\n  end\n\
end\n";

#[test]
fn a_call_through_an_interface_matches() {
    // The receiver's declared type is the *interface*, so the concrete class
    // is unknown at the call site and dispatch goes through the itable.
    must_agree(&format!(
        "{SHAPES}local s: Shape = Square(5)\ns.area()"
    ));
    must_agree(&format!(
        "{SHAPES}local s: Shape = Rect(3, 4)\ns.area()"
    ));
}

#[test]
fn one_call_site_dispatches_to_two_implementations() {
    // One `CALLIF` site, inside `areaOf`, reached from two classes whose
    // vtable layouts differ — which is exactly what the itable indirection
    // exists for.
    //
    // Deliberately free of arithmetic on the result: adding two call results
    // needs the dynamic `ARITHX` form, which is still to come (§21.4 item
    // 11), and that would make this a test about the wrong thing.
    let prog = format!("{SHAPES}fn areaOf(s: Shape) -> integer\n  return s.area()\nend\n");
    must_agree(&format!("{prog}areaOf(Square(5))"));
    must_agree(&format!("{prog}areaOf(Rect(3, 4))"));
}

#[test]
fn a_second_interface_method_resolves_to_its_own_slot() {
    must_agree(&format!(
        "{SHAPES}local s: Shape = Rect(2, 3)\ns.name()"
    ));
}

// ── the dynamic fallback ──────────────────────────────────────────────────

#[test]
fn arithmetic_on_untyped_call_results_matches() {
    // The gap `ARITHX` closes: adding two call results, where the front end
    // proved nothing about the operands. Before, this was refused outright.
    must_agree(
        "fn one() -> integer\n  return 1\nend\n\
         fn two() -> integer\n  return 2\nend\n\
         one() + two() * 10",
    );
}

#[test]
fn dynamic_arithmetic_matches_through_an_interface() {
    must_agree(&format!(
        "{SHAPES}fn total(a: Shape, b: Shape) -> integer\n  return a.area() + b.area()\nend\n\
         total(Square(5), Rect(3, 4))"
    ));
}

#[test]
fn dynamic_comparisons_and_concat_match() {
    must_agree("fn n() -> integer\n  return 3\nend\nn() < 5");
    must_agree("fn n() -> integer\n  return 3\nend\nn() >= 5");
    must_agree("fn s() -> string\n  return \"a\"\nend\ns() .. \"b\"");
}

#[test]
fn arithmetic_on_an_any_typed_operand_matches() {
    // `any` is the case §15.6 names outright: the checker proves nothing, so
    // only `ops::binary` can decide what `+` means here. Calling it rather
    // than reimplementing it is what keeps the answer — and any diagnostic —
    // identical to the tree-walker by construction.
    must_agree("local t: table<any> = {1, 2}
local x = t[1]
x");
}

#[test]
fn dynamic_division_by_zero_still_errors() {
    must_agree("fn z() -> integer\n  return 0\nend\nfn n() -> integer\n  return 1\nend\nn() / z()");
}

#[test]
fn an_operator_overload_dispatches_through_the_fallback() {
    // `Op*` overloads live only in `ops::binary`, so an instance operand is
    // exactly the case a typed opcode must not be chosen for.
    must_agree(
        "class V implements OpAdd\n\
         \x20 fn init(n: integer)\n    self.n = n\n  end\n  n: integer\n\
         \x20 fn add(other: V) -> V\n    return V(self.n + other.n)\n  end\n\
         end\n\
         local a = V(2)\n\
         local b = V(3)\n\
         local c = a + b\n\
         c.n",
    );
}

// ── try / catch / throw ───────────────────────────────────────────────────

#[test]
fn a_caught_throw_matches() {
    must_agree(
        "local r: string = \"none\"\n\
         try\n  throw \"boom\"\ncatch e: string\n  r = e\nend\nr",
    );
}

#[test]
fn a_try_that_does_not_throw_runs_no_handler() {
    // The happy path emits no instructions for the `try` at all.
    must_agree(
        "local r: integer = 0\n\
         try\n  r = 1\ncatch e: string\n  r = 2\nend\nr",
    );
}

#[test]
fn a_throw_from_a_called_function_unwinds_to_the_caller() {
    must_agree(
        "fn boom() -> nil\n  throw \"deep\"\nend\n\
         local r: string = \"none\"\n\
         try\n  boom()\ncatch e: string\n  r = e\nend\nr",
    );
}

#[test]
fn an_uncaught_throw_reports_the_same_way() {
    must_agree("throw \"escaped\"");
    must_agree("fn f() -> nil\n  throw \"from f\"\nend\nf()");
}

#[test]
fn a_catch_of_the_wrong_type_does_not_catch() {
    must_agree(
        "local r: integer = 0\n\
         try\n  throw \"a string\"\ncatch e: integer\n  r = 1\nend\nr",
    );
}

#[test]
fn nested_try_catches_at_the_inner_handler() {
    must_agree(
        "local r: string = \"none\"\n\
         try\n\
         \x20 try\n    throw \"inner\"\n  catch e: string\n    r = \"caught \" .. e\n  end\n\
         catch e2: string\n  r = \"outer\"\nend\nr",
    );
}

#[test]
fn a_loop_inside_a_try_still_works() {
    must_agree(
        "local s: integer = 0\n\
         try\n  for i = 1, 5 do s = s + i end\ncatch e: string\n  s = -1\nend\ns",
    );
}

// ── for … in ──────────────────────────────────────────────────────────────

#[test]
fn iterating_an_array_matches() {
    must_agree("local t: table<integer> = {10, 20, 30}\nlocal s: integer = 0\nfor v in t do s = s + v end\ns");
    must_agree(
        "local t: table<integer> = {10, 20, 30}\nlocal s: integer = 0\n\
         for i, v in t do s = s + i * v end\ns",
    );
}

#[test]
fn iterating_an_empty_table_runs_no_iterations() {
    must_agree("local t: table<integer> = {}\nlocal s: integer = 99\nfor v in t do s = 0 end\ns");
}

#[test]
fn break_and_continue_work_in_for_in() {
    must_agree(
        "local t: table<integer> = {1, 2, 3, 4, 5}\nlocal s: integer = 0\n\
         for v in t do\n  if v > 3 then break end\n  s = s + v\nend\ns",
    );
    must_agree(
        "local t: table<integer> = {1, 2, 3, 4, 5}\nlocal s: integer = 0\n\
         for v in t do\n  if v % 2 == 0 then continue end\n  s = s + v\nend\ns",
    );
}

#[test]
fn nested_for_in_matches() {
    must_agree(
        "local t: table<integer> = {1, 2, 3}\nlocal s: integer = 0\n\
         for a in t do\n  for b in t do\n    s = s + a * b\n  end\nend\ns",
    );
}

// ── nullability: `?.`, `??`, `!`, `as` (§15.12) ───────────────────────────

#[test]
fn coalesce_short_circuits_on_both_sides() {
    must_agree("local x: integer? = nil\nx ?? 7");
    must_agree("local x: integer? = 3\nx ?? 7");
    // Chained, and right-associative: the middle `??` only runs when the
    // first operand is nil.
    must_agree("local a: string? = nil\nlocal b: string? = nil\na ?? (b ?? \"last\")");
}

#[test]
fn coalesce_does_not_evaluate_a_present_left_operands_fallback() {
    // The fallback increments a counter, so evaluating it when it should
    // not have been shows up in the result rather than being invisible.
    must_agree(
        "local n: integer = 0\n\
         fn bump() -> integer\n  n = n + 1\n  return 99\nend\n\
         local x: integer? = 5\n\
         local y: integer = x ?? bump()\n\
         n",
    );
}

#[test]
fn force_unwrap_passes_a_value_and_throws_on_nil() {
    must_agree("local x: integer? = 4\nx! + 1");
    // The error text is compared too, so a divergent message fails here.
    must_agree("local x: integer? = nil\nx! + 1");
}

#[test]
fn a_cast_yields_the_value_or_nil() {
    // Bound to an annotated local rather than left as the module's result
    // expression: `a as integer ?? -1` alone is `UndeterminedType` to the
    // typechecker, so the bare form would not compile under *either* engine
    // and would be asserting nothing.
    must_agree("local a: any = 42\nlocal r: integer = a as integer ?? -1\nr");
    must_agree("local a: any = \"hi\"\nlocal r: integer = a as integer ?? -1\nr");
    must_agree("local a: any = \"hi\"\nlocal s: string? = a as string\nlocal r: string = s!\nr");
    must_agree("local a: any = 1.5\nlocal r: float = a as float ?? 0.0\nr");
    must_agree("local a: any = true\nlocal r: boolean = a as boolean ?? false\nr");
}

#[test]
fn a_cast_to_a_table_type_is_checked_elementwise() {
    // The reason `CASTCHK` carries a `Type` and calls the tree-walker's own
    // `cast`: a shallow "is it a table" test would say `true` to all three.
    must_agree("local a: any = {1, 2, 3}\nlocal r: boolean = a as table<integer> != nil\nr");
    must_agree("local a: any = {1, 2, 3}\nlocal r: boolean = a as table<string> != nil\nr");
    // An empty table satisfies any element type vacuously.
    must_agree("local a: any = {}\nlocal r: boolean = a as table<string> != nil\nr");
}

#[test]
fn a_cast_to_a_class_walks_the_inheritance_chain() {
    // A `Dog` is an `Animal`; a `Cat` is not.
    must_agree(
        "class Animal\n  fn speak() -> integer\n    return 1\n  end\nend\n\
         class Dog extends Animal\n  fn speak() -> integer\n    return 2\n  end\nend\n\
         local a: any = Dog()\n\
         local r: boolean = a as Animal != nil\nr",
    );
    must_agree(
        "class Animal\n  fn speak() -> integer\n    return 1\n  end\nend\n\
         class Cat\n  fn speak() -> integer\n    return 3\n  end\nend\n\
         local a: any = Cat()\n\
         local r: boolean = a as Animal != nil\nr",
    );
}

#[test]
fn a_safe_member_read_yields_nil_rather_than_faulting() {
    must_agree(
        "class Box\n  label: string = \"here\"\nend\n\
         local b: Box? = nil\n\
         b?.label ?? \"no Box\"",
    );
    must_agree(
        "class Box\n  label: string = \"here\"\nend\n\
         local b: Box? = Box()\n\
         b?.label ?? \"no Box\"",
    );
}

#[test]
fn a_safe_member_read_reaches_an_instance_field() {
    must_agree(
        "class P\n  fn init(h: integer)\n    self.health = h\n  end\n  health: integer\nend\n\
         local p: P? = P(7)\n\
         p?.health ?? -1",
    );
}

#[test]
fn a_safe_method_call_skips_its_arguments_when_the_receiver_is_nil() {
    // Not an optimisation: the tree-walker returns before evaluating the
    // arguments, so evaluating them here would run side effects it does not.
    must_agree(
        "class G\n  fn twice(n: integer) -> integer\n    return n * 2\n  end\nend\n\
         local calls: integer = 0\n\
         fn arg() -> integer\n  calls = calls + 1\n  return 5\nend\n\
         local g: G? = nil\n\
         local r: integer? = g?.twice(arg())\n\
         calls",
    );
    must_agree(
        "class G\n  fn twice(n: integer) -> integer\n    return n * 2\n  end\nend\n\
         local g: G? = G()\n\
         g?.twice(21) ?? 0",
    );
}

#[test]
fn a_static_read_through_an_instance_matches() {
    // A defaulted field on a class with no `init` is a *static* in both
    // engines, so `b.label` is a static read reached through a value.
    must_agree(
        "class Box\n  label: string = \"here\"\nend\n\
         local b: Box = Box()\n\
         b.label",
    );
}

#[test]
fn an_inherited_static_reads_the_slot_its_parent_declared() {
    // Static storage is one vector per class index. Resolving `Derived.total`
    // against `Derived` would address a second, never-initialized cell and
    // read `nil`.
    must_agree(
        "class Base\n  static total: integer = 7\nend\n\
         class Derived extends Base\nend\n\
         Derived.total",
    );
    must_agree(
        "class Base\n  static total: integer = 7\nend\n\
         class Derived extends Base\nend\n\
         Derived.total = 9\n\
         Base.total",
    );
}

// ── inherited vtable slots and operator overloads ─────────────────────────

#[test]
fn an_inherited_method_the_subclass_did_not_override_dispatches() {
    // Pass 1 copies the parent's vtable to extend its *numbering*, but at
    // that point no body is compiled, so what it copies is a row of
    // placeholders. Without the inheritance sweep this reported "`Child` has
    // no method in vtable slot 2".
    must_agree(
        "class Shape\n\
         \x20 fn area() -> integer\n    return 1\n  end\n\
         \x20 fn describe() -> integer\n    return self.area() * 10\n  end\n\
         end\n\
         class Circle extends Shape\n\
         \x20 fn area() -> integer\n    return 7\n  end\n\
         end\n\
         local c: Circle = Circle()\n\
         c.describe()",
    );
}

#[test]
fn a_unary_overload_dispatches() {
    // `ops::unary` looks the overload up on the runtime `ClassObject`, whose
    // method map is empty for a VM-built class — so this has to be resolved
    // at compile time, exactly like the binary overloads.
    must_agree(
        "class Money implements OpNeg, OpLen\n\
         \x20 fn init(c: integer)\n    self.cents = c\n  end\n\
         \x20 cents: integer\n\
         \x20 fn get() -> integer\n    return self.cents\n  end\n\
         \x20 fn neg() -> Money\n    return Money(-self.cents)\n  end\n\
         \x20 fn len() -> integer\n    return 4\n  end\n\
         end\n\
         local m: Money = Money(300)\n\
         local n: Money = -m\n\
         n.get()",
    );
    must_agree(
        "class Money implements OpLen\n\
         \x20 fn init(c: integer)\n    self.cents = c\n  end\n\
         \x20 cents: integer\n\
         \x20 fn len() -> integer\n    return 4\n  end\n\
         end\n\
         local m: Money = Money(300)\n\
         #m",
    );
}

#[test]
fn compare_and_equals_overloads_produce_the_operators_answer() {
    // `compare` returns an integer and `equals` a value read for
    // truthiness — neither is the operator's result. Using the raw return
    // made `b < a` evaluate to `-180`.
    let cls = "class Money implements OpEq, OpCompare\n\
         \x20 fn init(c: integer)\n    self.cents = c\n  end\n\
         \x20 cents: integer\n\
         \x20 fn equals(other: Money) -> boolean\n    return self.cents == other.cents\n  end\n\
         \x20 fn compare(other: Money) -> integer\n    return self.cents - other.cents\n  end\n\
         end\n\
         local a: Money = Money(300)\n\
         local b: Money = Money(120)\n";
    for op in ["<", "<=", ">", ">=", "==", "!="] {
        must_agree(&format!("{cls}local r: boolean = b {op} a\nr"));
        must_agree(&format!("{cls}local r: boolean = a {op} a\nr"));
    }
}

#[test]
fn index_overloads_read_and_write() {
    must_agree(
        "class Config implements OpIndex, OpNewIndex\n\
         \x20 fn init()\n    self.store = {}\n  end\n\
         \x20 store: table<string, string>\n\
         \x20 fn index(key: string) -> string\n    return self.store[key] ?? \"(unset)\"\n  end\n\
         \x20 fn newIndex(key: string, value: string)\n    self.store[key] = value\n  end\n\
         end\n\
         local c: Config = Config()\n\
         c[\"host\"] = \"localhost\"\n\
         local r: string = c[\"host\"] .. \"/\" .. c[\"missing\"]\n\
         r",
    );
}
