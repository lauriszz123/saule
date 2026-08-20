//! Multi-return and parallel binding (§6.3).

use crate::harness::*;

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


