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
    // A **tuple pattern** is the construct standing in for that here —
    // repoint this at another unsupported one when they land, the same way
    // the `unimplemented_opcodes_report_rather_than_panic` canary is.
    // It has already moved twice, from `import` and then from `a pipe`, as
    // each of those landed.
    // The assertion is about the *shape* of the refusal: it names the
    // construct and carries a span, so the CLI can fall back and say why.
    //
    // It used to point at `import`, which no longer refuses when a program
    // driver resolved it (`saule_vm::program::compile`). A lone `import`
    // compiled through this single-module path still does — see
    // `an_import_without_a_program_driver_still_refuses` below, which pins
    // that separately because it is a correctness rule rather than a
    // stand-in.
    let src = "fn divmod(a: integer, b: integer) -> (integer, integer)\n\
               \x20 return a / b, a % b\n\
               end\n\
               local r: string = match divmod(7, 2)\n\
               \x20 case (q, 0) then \"clean\"\n\
               \x20 case _ then \"rem\"\n\
               end\nr";
    let module = front_end(src);
    match saule_vm::compile(&module, "x.sau", src) {
        Err(saule_vm::CompileError::Unsupported { thing, span }) => {
            assert_eq!(thing, "a tuple pattern");
            assert!(span.start < span.end, "the refusal must point somewhere");
        }
        other => panic!("expected a clean Unsupported, got {other:?}"),
    }
}

#[test]
fn an_import_without_a_program_driver_still_refuses() {
    // Compiling one module on its own cannot bind an imported name: the
    // resolver gives it a module slot, and nothing would ever write to that
    // slot. Emitting a `GETMOD` against it would read `nil` — a wrong
    // answer with no symptom. Only `program::compile`, which resolves the
    // whole import graph first, may compile an `import` to nothing.
    let src = "import Json from \"json\"\n1";
    let module = front_end(src);
    match saule_vm::compile(&module, "x.sau", src) {
        Err(saule_vm::CompileError::Unsupported { thing, .. }) => {
            assert_eq!(thing, "an import declaration");
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

// ── stdlib constants and table dot access ─────────────────────────────────

#[test]
fn a_stdlib_constant_folds_to_the_same_value() {
    // `Math.ceil` already resolved to its native at compile time; these are
    // the members that hold a *value* rather than a function.
    must_agree("local r: float = Math.pi\nr");
    must_agree("local r: float = Math.e\nr");
    must_agree("local r: float = Math.sin(Math.pi / 2.0)\nr");
    must_agree("local r: string = Os.sep\nr");
}

#[test]
fn a_stdlib_enum_variant_folds() {
    // Unannotated: `local m: IoMode = ...` is `UndeterminedType` today —
    // the typechecker does not infer a stdlib enum's type from its variant.
    must_agree("local r: string = tostring(IoMode.Write)\nr");
}

#[test]
fn a_reassigned_stdlib_constant_is_not_folded() {
    // `Math.pi = 3.0` is accepted — the typechecker does not reject it — so
    // folding the read would freeze a value the program then changes.
    //
    // Today the *write* is itself unsupported, so the module falls back and
    // the no-fold guard never fires. That makes this a canary rather than a
    // behavioural test: when writes through a prelude receiver start
    // compiling, this fails, and at that point the guard is what keeps the
    // read honest.
    assert!(
        !agree("Math.pi = 3.0\nlocal r: float = Math.pi\nr"),
        "the write compiles now — check that the constant fold still declines \
         for a receiver this module assigns through"
    );
}

#[test]
fn a_top_level_local_shadowing_a_stdlib_name_wins() {
    // A module-level `local` becomes a module *slot*, not a frame local, so
    // `FuncCtx::lookup` cannot see it. Resolving these names on that lookup
    // alone read the stdlib's `pi` and called the stdlib's `String.len`
    // where the program meant its own table.
    must_agree(
        "local Math: table<string, float> = {pi: 3.0}\n\
         local r: float = Math.pi\nr",
    );
    must_agree(
        "local String: table<string, integer> = {len: 42}\n\
         local r: integer = String.len\nr",
    );
}

#[test]
fn a_top_level_local_shadowing_a_class_name_wins() {
    // Same failure, reached through the class-static path rather than the
    // prelude one.
    must_agree(
        "class Foo\n  static tag: integer = 1\nend\n\
         local Foo: table<string, integer> = {tag: 99}\n\
         local r: integer = Foo.tag\nr",
    );
}

#[test]
fn table_dot_access_reads_and_writes() {
    // `t.foo` is `t["foo"]`, and a miss is `nil` rather than an error.
    must_agree(
        "local t: table<string, string> = {}\n\
         t.name = \"alice\"\n\
         local r: string = t.name\nr",
    );
    must_agree(
        "local t: table<string, string> = {name: \"bob\"}\n\
         local r: string = t.name\nr",
    );
    must_agree(
        "local t: table<string, string> = {}\n\
         local r: string? = t.missing\n\
         local s: string = r ?? \"(nil)\"\ns",
    );
    // The dotted and bracketed spellings must agree with each other too.
    must_agree(
        "local t: table<string, integer> = {}\n\
         t.a = 1\n\
         t[\"b\"] = 2\n\
         local r: integer = t.a + t[\"b\"] + t[\"a\"] + t.b\nr",
    );
}

#[test]
fn table_dot_access_past_the_eight_bit_constant_window() {
    // `GETMAPK`/`SETMAPK` hold the key's constant index in an 8-bit operand.
    // Past 255 the compiler materialises the key and uses `GETIDX`/`SETIDX`
    // instead — the alternative would be capping a module at 256 constants
    // on an operation as ordinary as `t.name`.
    let mut src = String::from("local t: table<string, integer> = {}\nlocal pad: integer = 0\n");
    for i in 0..300 {
        src.push_str(&format!("pad = pad + {}\n", 1000 + i));
    }
    src.push_str("t.late = 7\nlocal r: integer = t.late\nr");
    must_agree(&src);
}

// ── Re-entrancy: the tree-walker calling into bytecode ───────────────────
//
// One root cause with several symptoms. `Value::VmFunction` used to be
// uncallable from `saule-interpreter` and a VM-built `ClassObject` used to
// carry an empty method map, so every path where the tree-walker's *own*
// code has to call a user function on a value hit a wall. Each test below
// is one of those paths. All of them were guarded by a compile-time refusal
// before; a refusal makes the engines agree by not running the VM at all,
// which is why `must_agree` — which fails if the compiler declines — is the
// right assertion here rather than `agree`.

#[test]
fn a_native_invokes_a_bytecode_comparator() {
    // `Table.sort`'s comparator, the case that kept `sort.sau` on the
    // tree-walker. The native calls `call_value_multi`, which now has an
    // arm that runs a fresh `Vm` over the caller's shared state.
    must_agree(
        "local t: table<integer, integer> = {5, 3, 9, 1}\n\
         Table.sort(t, (a: integer, b: integer) => a < b)\n\
         local r: string = t[1] .. \",\" .. t[2] .. \",\" .. t[3] .. \",\" .. t[4]\nr",
    );
    // Descending, so a comparator that is actually consulted is the only
    // way to pass: a no-op callback would leave the ascending order above.
    must_agree(
        "local t: table<integer, integer> = {5, 3, 9, 1}\n\
         Table.sort(t, (a: integer, b: integer) => a > b)\n\
         local r: string = t[1] .. \",\" .. t[2] .. \",\" .. t[3] .. \",\" .. t[4]\nr",
    );
}

#[test]
fn a_comparator_closure_captures_its_environment() {
    // The callback runs on a *fresh* register file but over the same shared
    // half, and it reaches its captured `flip` through an upvalue cell that
    // outlived the frame that created it.
    must_agree(
        "local flip: boolean = true\n\
         local cmp = fn(a: integer, b: integer) -> boolean\n\
         \x20 if flip then\n\
         \x20   return a > b\n\
         \x20 end\n\
         \x20 return a < b\n\
         end\n\
         local t: table<integer, integer> = {2, 8, 4}\n\
         Table.sort(t, cmp)\n\
         local r: string = t[1] .. \",\" .. t[2] .. \",\" .. t[3]\nr",
    );
}

#[test]
fn a_tostring_overload_is_honoured_by_concatenation() {
    // The worst failure this project could ship, and the one that was live:
    // `display_value` asked the class for a `toString`, a VM-built class
    // answered no, and `..` printed `<instance of Money>` — **with no
    // error**. Caught by `SAULE_DIFF=1`, not by any exit status.
    let money = "class Money implements OpToString\n\
                 \x20 local amount: integer\n\
                 \x20 fn init(a: integer)\n\
                 \x20   self.amount = a\n\
                 \x20 end\n\
                 \x20 fn toString() -> string\n\
                 \x20   return \"$\" .. self.amount\n\
                 \x20 end\n\
                 end\n";
    must_agree(&format!(
        "{money}local m: Money = Money(7)\nlocal r: string = \"cost: \" .. m\nr"
    ));
    must_agree(&format!(
        "{money}local m: Money = Money(7)\nlocal r: string = tostring(m)\nr"
    ));
    // Nested in a table, the *structural* rendering wins — `display_value`
    // applies to the value itself, not to values inside a table. The two
    // engines have to agree about that boundary too.
    must_agree(&format!(
        "{money}local t: table<integer, Money> = {{Money(1)}}\n\
         local r: string = \"\" .. t[1]\nr"
    ));
}

#[test]
fn a_tostring_overload_runs_exactly_once_per_operand() {
    // `CONCAT` used to render each operand twice — once to measure the
    // result's length, once to build it. Harmless while rendering was pure;
    // an overload is user code, so a second pass would run its side effects
    // twice. Counted rather than inferred.
    must_agree(
        "class Loud implements OpToString\n\
         \x20 static calls: integer = 0\n\
         \x20 fn toString() -> string\n\
         \x20   Loud.calls = Loud.calls + 1\n\
         \x20   return \"x\"\n\
         \x20 end\n\
         end\n\
         local a: Loud = Loud()\n\
         local b: Loud = Loud()\n\
         local s: string = a .. \"-\" .. b\n\
         local r: integer = Loud.calls\nr",
    );
}

#[test]
fn an_operator_overload_resolves_on_an_unproved_receiver() {
    // When the front end proved the operand's class the compiler picks the
    // overload itself. When it did not — a call result, here — `ARITHX`
    // falls through to `ops::binary`, which looks the overload up on the
    // runtime class. That lookup is the one that used to find an empty map.
    must_agree(
        "class Money implements OpAdd<Money, Money>, OpToString\n\
         \x20 local amount: integer\n\
         \x20 fn init(a: integer)\n\
         \x20   self.amount = a\n\
         \x20 end\n\
         \x20 fn add(other: Money) -> Money\n\
         \x20   return Money(self.amount + other.amount)\n\
         \x20 end\n\
         \x20 fn toString() -> string\n\
         \x20   return \"$\" .. self.amount\n\
         \x20 end\n\
         end\n\
         fn make(n: integer) -> Money\n\
         \x20 return Money(n)\n\
         end\n\
         local r: string = \"\" .. (make(2) + make(40))\nr",
    );
}

#[test]
fn an_inherited_method_is_reachable_through_the_runtime_class() {
    // The method map the VM builds comes from `vindex` and `vtable`, both
    // of which are prefix-extensions of the parent's — so an inherited,
    // non-overridden `toString` is one probe away, exactly as it is on a
    // tree-walker class. Copying only a class's *own* methods would leave
    // this one unreachable and silently fall back to `<instance of Dog>`.
    must_agree(
        "class Animal implements OpToString\n\
         \x20 fn toString() -> string\n\
         \x20   return \"an animal\"\n\
         \x20 end\n\
         end\n\
         class Dog extends Animal\n\
         end\n\
         local d: Dog = Dog()\n\
         local r: string = \"\" .. d\nr",
    );
}

#[test]
fn a_callback_can_itself_reach_back_into_the_vm() {
    // Two levels of re-entrancy: the outer comparator is called from a
    // native, and *it* calls a native that calls another comparator. Each
    // level is a fresh `Vm` over the same shared half, so this is where a
    // per-invocation piece wrongly left in `VmShared` — the register file,
    // the frame list, the open upvalues — would corrupt the level below it
    // rather than merely be slow.
    must_agree(
        "local inner: table<integer, integer> = {3, 1, 2}\n\
         fn outer(a: integer, b: integer) -> boolean\n\
         \x20 Table.sort(inner, (x: integer, y: integer) => x < y)\n\
         \x20 return a < b\n\
         end\n\
         local t: table<integer, integer> = {9, 4, 6}\n\
         Table.sort(t, outer)\n\
         local r: string = t[1] .. \",\" .. t[2] .. \",\" .. t[3] .. \"|\" ..\n\
         \x20 inner[1] .. \",\" .. inner[2] .. \",\" .. inner[3]\nr",
    );
}

#[test]
fn the_recursion_guard_still_unwinds_after_re_entrant_calls() {
    // A guard that leaked a level per callback would shrink every later
    // program's budget — and because the counter is per *thread*, the
    // symptom would surface as an unrelated test failing later in this
    // binary. Sorting drives many comparator calls, so a drift of one per
    // call would be obvious immediately after.
    //
    // The unbounded case — a comparator that sorts with itself forever —
    // is pinned by `tests/ui/stack_overflow_reentrant.sau` instead. It
    // needs `MAX_EVAL_DEPTH` native frames of real stack, which is more
    // than libtest's 2 MiB test thread has; that fixture runs `saule` as a
    // process, on a main thread, which is the configuration users get.
    must_agree(
        "local t: table<integer, integer> = {5, 2, 9, 1, 7, 3}\n\
         Table.sort(t, (a: integer, b: integer) => a < b)\n\
         fn depth(n: integer) -> integer\n\
         \x20 if n <= 0 then\n\
         \x20   return 0\n\
         \x20 end\n\
         \x20 return 1 + depth(n - 1)\n\
         end\n\
         depth(60)",
    );
}


// ── `match` guards ────────────────────────────────────────────────────────
//
// Two bugs lived here, and only one of them announced itself. The compiler
// emitted an arm's guard *before* entering the arm's scope, so a binding
// pattern's name was not in a register yet — `case x when x < 0` refused
// with "a local the compiler has not seen declared", which at least fell
// back safely. The second was silent: the pattern's failure jump was patched
// to just past the guard's jump, which is where the arm **body** starts, so
// a pattern that did not match ran the arm anyway.

#[test]
fn a_guard_can_read_the_binding_its_own_pattern_introduces() {
    // The refusal. `x` is bound by the pattern and read by the guard, and
    // the resolver binds both the same way — it was only the compiler that
    // had not put `x` in a register yet.
    must_agree(
        "fn classify(n: integer) -> string\n\
         \x20 return match n\n\
         \x20   case x when x < 0 then \"negative \" .. x\n\
         \x20   case 0 then \"zero\"\n\
         \x20   case x when x < 10 then \"small \" .. x\n\
         \x20   case x then \"big \" .. x\n\
         \x20 end\n\
         end\n\
         local r: string = classify(-5) .. \"|\" .. classify(0) .. \"|\"\n\
         \x20 .. classify(3) .. \"|\" .. classify(999)\nr",
    );
}

#[test]
fn a_failing_pattern_with_a_guard_does_not_fall_into_the_arm() {
    // The silent one. `0` does not match `5`, so the guard should never be
    // reached and the arm never taken — but the pattern's failure jump
    // landed at the top of the body, and the VM answered "zero".
    //
    // A wrong value, exit status 0, and no fixture had a literal pattern
    // with a guard, so nothing in the suite could see it.
    must_agree(
        "local n: integer = 5\n\
         local r: string = match n\n\
         \x20 case 0 when true then \"zero\"\n\
         \x20 case _ then \"other\"\n\
         end\nr",
    );
    // The same shape with the guard *false* on a pattern that does match,
    // so the arm is skipped for the other reason.
    must_agree(
        "local n: integer = 0\n\
         local r: string = match n\n\
         \x20 case 0 when false then \"zero\"\n\
         \x20 case _ then \"other\"\n\
         end\nr",
    );
    // And both failure paths in one match, so a mis-patched jump from
    // either arm shows up.
    must_agree(
        "fn pick(n: integer) -> string\n\
         \x20 return match n\n\
         \x20   case 1 when false then \"one-guarded\"\n\
         \x20   case 2 when true then \"two\"\n\
         \x20   case x when x > 10 then \"big\"\n\
         \x20   case _ then \"rest\"\n\
         \x20 end\n\
         end\n\
         local r: string = pick(1) .. \"|\" .. pick(2) .. \"|\" .. pick(11) .. \"|\" .. pick(3)\nr",
    );
}

#[test]
fn a_guard_can_read_a_variant_payload_binding() {
    // The same ordering rule for a destructured payload: `x` comes out of
    // the variant, and the guard must see it.
    must_agree(
        "enum Event\n\
         \x20 Click(x: integer, y: integer),\n\
         \x20 Key(code: string)\n\
         end\n\
         fn describe(e: Event) -> string\n\
         \x20 return match e\n\
         \x20   case Event.Click(x, y) when x > 0 then \"right \" .. x .. \",\" .. y\n\
         \x20   case Event.Click(x, y) then \"left \" .. x .. \",\" .. y\n\
         \x20   case Event.Key(c) then \"key \" .. c\n\
         \x20 end\n\
         end\n\
         local r: string = describe(Event.Click(-3, 7)) .. \"|\"\n\
         \x20 .. describe(Event.Click(4, 2)) .. \"|\" .. describe(Event.Key(\"a\"))\nr",
    );
}

// ── `for … in` over a closure driver (§15.8) ──────────────────────────────
//
// Lowered to an ordinary `CALL` in a `while` shape rather than taught to
// `ITERNEXT`: `CALL` already dispatches on whatever it finds — a bytecode
// closure, a native, a native closure — so the driver can be any of them
// with no new opcode. The result count is fixed at `nvars`, which is what
// makes "extras → nil, surplus dropped" fall out of `pop_frame` instead of
// needing its own rule.

#[test]
fn a_closure_drives_a_for_in_loop() {
    must_agree(
        "fn counter(stop: integer) -> fn() -> integer?\n\
         \x20 local i: integer = 0\n\
         \x20 return fn()\n\
         \x20   i = i + 1\n\
         \x20   if i > stop then return nil end\n\
         \x20   return i\n\
         \x20 end\n\
         end\n\
         local sum: integer = 0\n\
         for n in counter(4) do\n\
         \x20 sum = sum + n\n\
         end\n\
         sum",
    );
}

#[test]
fn a_driver_that_yields_nothing_runs_no_iterations() {
    // The case that decided the calling convention. Asking for *all*
    // results would leave the callee register holding the driver itself
    // when a step returned nothing — a function, not nil — and the loop
    // would never end. A fixed result count pads with nil instead.
    must_agree(
        "fn empty() -> fn() -> integer?\n\
         \x20 return fn()\n\
         \x20   return nil\n\
         \x20 end\n\
         end\n\
         local n: integer = 0\n\
         for x in empty() do\n\
         \x20 n = n + 1\n\
         end\n\
         n",
    );
}

#[test]
fn break_and_continue_work_inside_a_driver_loop() {
    // `continue` re-enters at the *call* — the next step is what advances
    // this loop, so there is no separate increment to jump to.
    must_agree(
        "fn counter(stop: integer) -> fn() -> integer?\n\
         \x20 local i: integer = 0\n\
         \x20 return fn()\n\
         \x20   i = i + 1\n\
         \x20   if i > stop then return nil end\n\
         \x20   return i\n\
         \x20 end\n\
         end\n\
         local sum: integer = 0\n\
         for n in counter(10) do\n\
         \x20 if n == 3 then continue end\n\
         \x20 if n == 6 then break end\n\
         \x20 sum = sum + n\n\
         end\n\
         sum",
    );
}

#[test]
fn driver_loops_nest() {
    // Each loop holds its driver in a register of its own; sharing one
    // would make the inner loop exhaust the outer one's.
    must_agree(
        "fn counter(stop: integer) -> fn() -> integer?\n\
         \x20 local i: integer = 0\n\
         \x20 return fn()\n\
         \x20   i = i + 1\n\
         \x20   if i > stop then return nil end\n\
         \x20   return i\n\
         \x20 end\n\
         end\n\
         local out: string = \"\"\n\
         for a in counter(2) do\n\
         \x20 for b in counter(2) do\n\
         \x20   out = out .. a .. b .. \" \"\n\
         \x20 end\n\
         end\n\
         out",
    );
}

// ── pipes ─────────────────────────────────────────────────────────────────
//
// `when(source):a(x):b(y)` lowers to a chain of ordinary calls, each
// threading the upstream value in as argument 0 — what `eval`'s `Expr::Pipe`
// arm does. The value lives in one register for the whole chain.
//
// The callee is resolved **by name**: a `PipeStage` holds a `String` and has
// no `NodeId`, so the binding table has nothing keyed on it and the lookup
// order is written out by hand. These pin that the hand-written order agrees
// with the resolver's.

#[test]
fn a_pipeline_threads_its_value_through_each_stage() {
    must_agree(
        "fn double(n: integer) -> integer\n  return n * 2\nend\n\
         local r: integer = when(4):double()\nr",
    );
    // Chained, so a stage reading a stale register would show up.
    must_agree(
        "fn double(n: integer) -> integer\n  return n * 2\nend\n\
         local r: integer = when(3):double():double():double()\nr",
    );
}

#[test]
fn a_pipeline_stage_takes_extra_arguments_after_the_piped_value() {
    // The piped value is argument 0 and the written ones follow, so an
    // off-by-one in the window would swap `a` and `b` here — and `add` is
    // commutative on purpose *not* chosen: `sub` would hide nothing.
    must_agree(
        "fn sub(a: integer, b: integer) -> integer\n  return a - b\nend\n\
         local r: integer = when(10):sub(3)\nr",
    );
    must_agree(
        "fn sub(a: integer, b: integer) -> integer\n  return a - b\nend\n\
         fn double(n: integer) -> integer\n  return n * 2\nend\n\
         local r: integer = when(10):sub(3):double():sub(1)\nr",
    );
}

// **Not** covered here, deliberately:
//
// * a *prelude* name as a stage — `saule-typeck` rejects
//   `when(x):tostring()` with `UnknownPipeStage`, so it never reaches a
//   valid program and the compiler has no branch for it;
// * a stage naming a `fn` declared *below* the pipeline at module level.
//   The two engines genuinely disagree there — and they disagree about a
//   plain `later(5)` written the same way, with no pipe involved, so it
//   predates this work. Written up in VM_TASKS.md rather than papered over
//   with a skipped test.

#[test]
fn a_pipeline_over_a_table_matches() {
    must_agree(
        "fn total(xs: table<integer>) -> integer\n\
         \x20 local s: integer = 0\n\
         \x20 for v in xs do\n\
         \x20   s = s + v\n\
         \x20 end\n\
         \x20 return s\n\
         end\n\
         local t: table<integer> = {5, 1, 4}\n\
         local r: integer = when(t):total()\nr",
    );
}


// ── class statics by bare name ────────────────────────────────────────────
//
// `Binding::ClassStatic` carries the *class name*, not a slot, because the
// answer has to survive a lambda nested inside the method — a different
// `FuncCtx` with no `current_class` of its own. Everything below turns on
// one rule: an inherited static lives in the cell its **declaring** class
// owns, so every reader and writer must address that one cell.

#[test]
fn a_static_is_read_and_written_by_its_bare_name_inside_a_method() {
    must_agree(
        "class Counter\n\
         \x20 static count: integer = 0\n\
         \x20 static fn bump()\n\
         \x20   count = count + 1\n\
         \x20 end\n\
         \x20 static fn get() -> integer\n\
         \x20   return count\n\
         \x20 end\n\
         end\n\
         Counter.bump()\n\
         Counter.bump()\n\
         local r: integer = Counter.get()\nr",
    );
}

#[test]
fn self_inside_a_static_method_reaches_the_class_statics() {
    // In a `static fn`, `self` is the class — `call_static_method_multi`
    // binds it to `Value::Class`. Resolved at compile time to a static
    // access, which is why the VM never needs a class in a register.
    must_agree(
        "class Counter\n\
         \x20 static count: integer = 0\n\
         \x20 static label: string = \"c\"\n\
         \x20 static fn bump()\n\
         \x20   self.count = self.count + 1\n\
         \x20 end\n\
         \x20 static fn describe() -> string\n\
         \x20   return self.label .. \"=\" .. self.count\n\
         \x20 end\n\
         end\n\
         Counter.bump()\n\
         Counter.bump()\n\
         local r: string = Counter.describe()\nr",
    );
}

#[test]
fn an_inherited_static_addresses_the_declaring_classes_cell() {
    // The §24.2 shape. `sindex` and `smindex` are both flattened and both
    // name the declaring class, so a subclass reading, writing, or calling
    // an inherited static reaches the parent's one cell — not a second,
    // never-initialised one of its own. Getting this wrong reads `nil`.
    must_agree(
        "class Entity\n\
         \x20 static maxHealth: integer = 100\n\
         \x20 static fn describe() -> string\n\
         \x20   return \"capped at \" .. self.maxHealth\n\
         \x20 end\n\
         end\n\
         class Player extends Entity\n\
         end\n\
         local r: string = Player.maxHealth .. \"|\" .. Player.describe()\nr",
    );
    // And a write through the subclass name is seen through the parent's.
    must_agree(
        "class Entity\n\
         \x20 static total: integer = 0\n\
         end\n\
         class Player extends Entity\n\
         end\n\
         Player.total = 7\n\
         local r: string = Entity.total .. \"|\" .. Player.total\nr",
    );
}

#[test]
fn a_private_static_fn_is_callable_by_bare_name_from_a_sibling() {
    // `static local fn` — a *method*, so it lives in `smindex` and not in
    // the `sindex` a bare-name static *read* consults. Without its own
    // arm it fell through to the generic call and asked for a static field
    // that does not exist.
    must_agree(
        "class Bank\n\
         \x20 static local secret: integer = 42\n\
         \x20 static local fn check(n: integer) -> boolean\n\
         \x20   return n == secret\n\
         \x20 end\n\
         \x20 static fn unlock(code: integer) -> string\n\
         \x20   if check(code) then\n\
         \x20     return \"opened\"\n\
         \x20   end\n\
         \x20   return \"denied\"\n\
         \x20 end\n\
         end\n\
         local r: string = Bank.unlock(42) .. \"|\" .. Bank.unlock(13)\nr",
    );
}

// ── §19 compile-time argument binding ─────────────────────────────────────
//
// Defaults become per-arity **entry stubs** in the callee, so a call that
// omits one just reports a shorter arity and `entry_for` lands on the stub
// that fills it. Named arguments are reordered at compile time; the runtime
// never sees a name.

#[test]
fn a_defaulted_parameter_is_filled_by_the_callee() {
    must_agree(
        "fn greet(intro: string, name: string = \"Unnamed\") -> string\n\
         \x20 return intro .. \" \" .. name\n\
         end\n\
         local r: string = greet(\"hi\") .. \"|\" .. greet(\"hi\", \"Lyra\")\nr",
    );
}

#[test]
fn a_default_is_evaluated_in_the_callees_frame() {
    // §19 calls this the one genuine correctness trap. Entry stubs get it
    // right by construction: `b`'s default compiles into the callee's
    // register for `b`, so `a` resolves to the callee's parameter — not to
    // whatever `a` happens to mean at the call site.
    must_agree(
        "local a: integer = 100\n\
         fn f(a: integer, b: integer = a * 2) -> integer\n\
         \x20 return b\n\
         end\n\
         local r: integer = f(3)\nr",
    );
}

#[test]
fn defaults_on_a_method_account_for_self() {
    // A method's parameters start at register 1, and its `self` counts as an
    // argument — so the entry table is indexed by *arity including self*.
    // Off by one here and a one-argument call would run the wrong stub.
    must_agree(
        "class Greeter\n\
         \x20 fn hello(name: string = \"world\") -> string\n\
         \x20   return \"hello \" .. name\n\
         \x20 end\n\
         end\n\
         local g: Greeter = Greeter()\n\
         local r: string = g.hello() .. \"|\" .. g.hello(\"you\")\nr",
    );
}

#[test]
fn named_arguments_are_reordered_at_compile_time() {
    must_agree(
        "fn sub(a: integer, b: integer) -> integer\n  return a - b\nend\n\
         local r: integer = sub(b: 3, a: 10)\nr",
    );
}

#[test]
fn a_named_argument_may_skip_a_nullable_parameter() {
    // The gap case: nothing fills slot 0, so the call passes an explicit
    // `nil` — which is what the callee would have left there anyway. A
    // skipped parameter with a *default* is refused instead, because the
    // default has to run in the callee and the stubs only fill a suffix.
    must_agree(
        "fn show(first: string?, second: string) -> string\n\
         \x20 return (first ?? \"-\") .. \"/\" .. second\n\
         end\n\
         local r: string = show(second: \"b\")\nr",
    );
}

#[test]
fn a_constructor_takes_named_arguments() {
    must_agree(
        "class Point\n\
         \x20 x: integer\n\
         \x20 y: integer\n\
         \x20 fn init(x: integer, y: integer)\n\
         \x20   self.x = x\n\
         \x20   self.y = y\n\
         \x20 end\n\
         end\n\
         local p: Point = Point(y: 2, x: 1)\n\
         local r: string = p.x .. \",\" .. p.y\nr",
    );
}

#[test]
fn a_variadic_parameter_gathers_the_surplus_arguments() {
    // Gathered by the **callee**, via `VARARG` as its first instruction —
    // not packed into a table at the call site. Packing at the call site
    // would have needed no new opcode, but only works where the caller can
    // *see* that the callee is variadic: not through a function value, and
    // not across a module boundary.
    let sum = "fn total(...values: integer) -> integer\n\
               \x20 local s: integer = 0\n\
               \x20 for v in values do\n\
               \x20   s = s + v\n\
               \x20 end\n\
               \x20 return s\n\
               end\n";
    must_agree(&format!("{sum}local r: integer = total(1, 2, 3, 4)\nr"));
    // No surplus at all: the parameter must still be an empty *table*, not
    // nil, or `#values` and `for … in` would both fault.
    must_agree(&format!("{sum}local r: integer = total()\nr"));
    must_agree(&format!("{sum}local r: integer = total(7)\nr"));
}

#[test]
fn a_variadic_parameter_follows_fixed_ones() {
    must_agree(
        "fn tag(label: string, ...rest: integer) -> string\n\
         \x20 local s: integer = 0\n\
         \x20 for v in rest do\n\
         \x20   s = s + v\n\
         \x20 end\n\
         \x20 return label .. \"=\" .. s\n\
         end\n\
         local r: string = tag(\"a\", 1, 2) .. \"|\" .. tag(\"b\")\nr",
    );
}

// ── §8.5 dynamic member dispatch ──────────────────────────────────────────
//
// `GETFX` and `CALLMX` are the escape hatch for a receiver whose class the
// front end did not prove. Both defer to the tree-walker's own member logic
// — the same reuse rule `ARITHX` follows — so every receiver kind behaves
// identically without the compiler learning each one.

#[test]
fn a_member_read_on_an_unproved_receiver_matches() {
    must_agree(
        "class Box\n  v: integer\n  fn init(v: integer)\n    self.v = v\n  end\nend\n\
         local b: any = Box(7)\n\
         local r: any = b.v\nr",
    );
}

#[test]
fn a_method_call_on_an_unproved_receiver_matches() {
    must_agree(
        "class Box\n\
         \x20 v: integer\n\
         \x20 fn init(v: integer)\n    self.v = v\n  end\n\
         \x20 fn doubled() -> integer\n    return self.v * 2\n  end\n\
         end\n\
         local b: any = Box(7)\n\
         b.doubled()",
    );
}

#[test]
fn a_missing_member_on_an_unproved_receiver_fails_the_same_way() {
    // The error text has to match too, which is the whole reason this
    // defers to `read_member` rather than reimplementing the lookup.
    must_agree(
        "class Box\n  v: integer\n  fn init(v: integer)\n    self.v = v\n  end\nend\n\
         local b: any = Box(1)\n\
         local r: any = b.nope\nr",
    );
}

#[test]
fn an_enum_variants_value_falls_back_to_its_name() {
    // A variant with no declared value answers `.value` with its own name,
    // not nil. `UNWRAP` returned nil until `GETFX` let `enums.sau` compile
    // and `SAULE_DIFF=1` put the two engines side by side.
    must_agree(
        "enum Direction\n  North,\n  South\nend\n\
         local d: Direction = Direction.North\n\
         local r: string = d.value .. \"/\" .. d.name\nr",
    );
    // And a variant that *does* declare one still answers with it.
    must_agree(
        "enum Status\n  Alive = \"alive\",\n  Dead = \"dead\"\nend\n\
         local s: Status = Status.Alive\n\
         local r: string = s.value .. \"/\" .. s.name\nr",
    );
}

// ── §6.3 multi-return and parallel binding ────────────────────────────────
//
// The rule being reproduced is `eval_expr_list`'s, and it is narrower than
// it first looks: **only the last expression of a list expands**, and only
// when it is a call — `eval_values` matches `Expr::Call` and hands back a
// one-element list for everything else. Extra names become nil, surplus
// values are dropped *after* being evaluated.

#[test]
fn a_parallel_local_takes_both_results_of_a_call() {
    must_agree(
        "fn pair() -> (integer, integer)\n\
         \x20 return 11, 22\n\
         end\n\
         local a: integer, b: integer = pair()\n\
         local r: string = a .. \"/\" .. b\nr",
    );
}

#[test]
fn a_parallel_local_of_plain_values_binds_positionally() {
    must_agree(
        "local a: integer, b: integer = 1, 2\n\
         local r: string = a .. \"/\" .. b\nr",
    );
}

#[test]
fn a_parallel_local_pads_missing_values_with_nil() {
    // Three names, one two-valued call: the third is nil rather than a
    // register the callee never wrote — which is what a `C` operand of
    // `nret + 1` buys, since `pop_frame` fills the shortfall.
    must_agree(
        "fn pair() -> (integer, integer)\n\
         \x20 return 11, 22\n\
         end\n\
         local a: integer?, b: integer?, c: integer? = pair()\n\
         local r: string = tostring(a) .. \"/\" .. tostring(b) .. \"/\" .. tostring(c)\nr",
    );
    must_agree(
        "local a: integer?, b: integer? = 1\n\
         local r: string = tostring(a) .. \"/\" .. tostring(b)\nr",
    );
}

#[test]
fn a_surplus_value_is_still_evaluated_before_it_is_dropped() {
    // Dropping a value is not the same as not producing it. Counted rather
    // than inferred, because the compiler *could* have skipped emitting the
    // expression entirely and nothing else would have noticed.
    must_agree(
        "local calls: integer = 0\n\
         fn bump() -> integer\n\
         \x20 calls = calls + 1\n\
         \x20 return 9\n\
         end\n\
         local a: integer, b: integer = 1, 2, bump()\n\
         local r: string = a .. \"/\" .. b .. \"/\" .. calls\nr",
    );
}

#[test]
fn only_the_last_expression_of_a_list_expands() {
    // `pair()` in a non-final position contributes exactly one value, so
    // `b` is 7 and not `pair()`'s second result.
    must_agree(
        "fn pair() -> (integer, integer)\n\
         \x20 return 11, 22\n\
         end\n\
         local a: integer, b: integer = pair(), 7\n\
         local r: string = a .. \"/\" .. b\nr",
    );
}

#[test]
fn a_parallel_assignment_evaluates_the_whole_right_side_first() {
    // The swap is the point: writing targets as they are computed would
    // leave both names holding the same value.
    must_agree(
        "local a: integer = 1\n\
         local b: integer = 2\n\
         a, b = b, a\n\
         local r: string = a .. \"/\" .. b\nr",
    );
    // Fibonacci's shape — the right-hand side reads the *old* `a`.
    must_agree(
        "fn fib(n: integer) -> integer\n\
         \x20 local a: integer, b: integer = 0, 1\n\
         \x20 for i: integer = 2, n do\n\
         \x20   a, b = b, a + b\n\
         \x20 end\n\
         \x20 return b\n\
         end\n\
         fib(10)",
    );
}

#[test]
fn a_parallel_assignment_writes_fields_and_table_slots() {
    must_agree(
        "class P\n\
         \x20 x: integer\n\
         \x20 y: integer\n\
         \x20 fn init(x: integer, y: integer)\n    self.x = x\n    self.y = y\n  end\n\
         end\n\
         local p: P = P(1, 2)\n\
         p.x, p.y = p.y, p.x\n\
         local r: string = p.x .. \"/\" .. p.y\nr",
    );
    must_agree(
        "local t: table<integer> = {1, 2}\n\
         t[1], t[2] = t[2], t[1]\n\
         local r: string = t[1] .. \"/\" .. t[2]\nr",
    );
}

#[test]
fn return_passes_every_result_of_a_call_through() {
    // **The divergence this slice was written to close.** `return f()` under
    // the tree-walker returns all of `f`'s values; the VM compiled `RET1`
    // and truncated to one. Invisible until something consumed more than
    // one — exit status 0, wrong value, which is the failure mode this
    // project treats as the worst it can ship.
    must_agree(
        "fn pair() -> (integer, integer)\n\
         \x20 return 11, 22\n\
         end\n\
         fn wrap() -> (integer, integer)\n\
         \x20 return pair()\n\
         end\n\
         local a: integer, b: integer = wrap()\n\
         local r: string = a .. \"/\" .. b\nr",
    );
}

#[test]
fn a_returned_call_still_yields_one_value_where_it_should() {
    // The other side of the same change: a single-valued callee passed
    // through must not start producing a second nil.
    must_agree(
        "fn one() -> integer\n  return 5\nend\n\
         fn wrap() -> integer\n  return one()\nend\n\
         local a: integer?, b: integer? = wrap()\n\
         local r: string = tostring(a) .. \"/\" .. tostring(b)\nr",
    );
    // A constructor is single-valued too, and it reaches `return` through a
    // different path — one that writes its result to a register rather than
    // leaving it in a call window.
    must_agree(
        "class Box\n  v: integer\n  fn init(v: integer)\n    self.v = v\n  end\nend\n\
         fn make() -> Box\n  return Box(3)\nend\n\
         local b: Box = make()\n\
         b.v",
    );
}

#[test]
fn a_method_call_yields_both_of_its_results() {
    // `CALLM` carries its vtable slot in `C`, so it can only ever return
    // one value; anything else is `CALLM_MR`, with the slot displaced into
    // `EXTRAARG`. Both forms are exercised here — the second call wants one
    // result and must still take the cheap opcode.
    must_agree(
        "class Split\n\
         \x20 n: integer\n\
         \x20 fn init(n: integer)\n    self.n = n\n  end\n\
         \x20 fn halves() -> (integer, integer)\n    return self.n / 2, self.n % 2\n  end\n\
         end\n\
         local s: Split = Split(7)\n\
         local q: integer, rem: integer = s.halves()\n\
         local one: integer = s.halves()\n\
         local r: string = q .. \"/\" .. rem .. \"/\" .. one\nr",
    );
}

#[test]
fn an_interface_calls_results_pass_through_a_return() {
    // `CALLIF`'s `C` is the interface's method slot, so its result count
    // rides packed into `EXTRAARG` beside the interface index. `return
    // s.area()` is what makes that live: it asks for *all* results, and
    // there was nowhere to say so before.
    //
    // A parallel `local` from an interface call would exercise the same
    // encoding but cannot be written yet — `saule-typeck` reports `cannot
    // determine the type of this expression` for **any** interface method
    // call's return type, single-valued ones included, so a `return` is the
    // only reachable consumer.
    must_agree(
        "interface Shape\n  fn area() -> integer\nend\n\
         class Square implements Shape\n\
         \x20 s: integer\n\
         \x20 fn init(s: integer)\n    self.s = s\n  end\n\
         \x20 fn area() -> integer\n    return self.s * self.s\n  end\n\
         end\n\
         fn areaOf(s: Shape) -> integer\n  return s.area()\nend\n\
         areaOf(Square(6))",
    );
}

#[test]
fn a_native_yields_both_of_its_results() {
    // `String.find` returns start and end. It compiles to `CALLNAT`, whose
    // results come back through `store_results` rather than `pop_frame` —
    // a different padding path, and one a bytecode-only test would miss.
    must_agree(
        "local s: integer?, e: integer? = String.find(\"hello world\", \"world\")\n\
         local r: string = tostring(s) .. \"/\" .. tostring(e)\nr",
    );
    must_agree(
        "local s: integer?, e: integer? = String.find(\"hello\", \"zzz\")\n\
         local r: string = tostring(s) .. \"/\" .. tostring(e)\nr",
    );
}

#[test]
fn a_module_level_parallel_local_writes_module_slots() {
    // A `local` at the top of the module body is a module *slot*, not a
    // frame register (§0.6) — the distinction three earlier bugs came from.
    must_agree(
        "fn pair() -> (integer, integer)\n\
         \x20 return 4, 5\n\
         end\n\
         local a: integer, b: integer = pair()\n\
         fn sum() -> integer\n  return a + b\nend\n\
         sum()",
    );
}

#[test]
fn a_parallel_local_from_a_lambda_call_matches() {
    // The generic `CALL`, where the callee is a value rather than a proto
    // the compiler resolved.
    must_agree(
        "local f: fn() -> (integer, integer) = fn() return 8, 9 end\n\
         local a: integer, b: integer = f()\n\
         local r: string = a .. \"/\" .. b\nr",
    );
}

#[test]
fn a_returned_call_through_a_driver_yields_every_value() {
    // The shape that proves the point: a `for … in` driver asks for exactly
    // `nvars` results, so a driver whose body is `return inner()` is the
    // one place a truncating `RET1` produced a wrong *value* rather than a
    // refusal. It printed `nil` for the second variable.
    must_agree(
        "fn pair() -> (integer, integer)\n\
         \x20 return 11, 22\n\
         end\n\
         fn wrap() -> (integer, integer)\n\
         \x20 return pair()\n\
         end\n\
         fn mkdriver() -> fn() -> (integer, integer)\n\
         \x20 local done: boolean = false\n\
         \x20 return fn()\n\
         \x20   if done then return nil end\n\
         \x20   done = true\n\
         \x20   return wrap()\n\
         \x20 end\n\
         end\n\
         local out: string = \"\"\n\
         for a, b in mkdriver() do\n\
         \x20 out = out .. a .. \"/\" .. b\n\
         end\n\
         out",
    );
}

#[test]
fn passed_through_results_may_outnumber_the_frame_that_carries_them() {
    // `wrap` needs two registers of its own, and eight values land in it on
    // the way through. That is legal precisely because the call window is
    // the *top* of the register file and the callee's frame has already been
    // popped, so the overflow lands on stack nobody else owns — but it is
    // the one place `max_regs` stops being an upper bound on what a frame
    // touches, so it is asserted rather than assumed.
    must_agree(
        "fn many() -> (integer, integer, integer, integer, integer, integer, integer, integer)
           return 1, 2, 3, 4, 5, 6, 7, 8
         end
         fn wrap() -> (integer, integer, integer, integer, integer, integer, integer, integer)
           return many()
         end
         local a: integer, b: integer, c: integer, d: integer, e: integer, f: integer, g: integer, h: integer = wrap()
         local r: string = a .. b .. c .. d .. e .. f .. g .. h
r",
    );
}
