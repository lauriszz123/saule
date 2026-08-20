//! The dynamic fallback: unproved receivers, member dispatch, and the write side (§8.5).

use crate::harness::*;

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


// ── §8.5 the dynamic escape hatch, nullable and write sides ───────────────
//
// `GETFX`/`CALLMX` gave an ordinary member read and call a fallback for a
// receiver the front end did not prove. The *nullable* read and call, and
// the member **write**, kept refusing — which is backwards: a nullable
// receiver is if anything more likely to be unproved than a plain one.

#[test]
fn a_safe_member_read_on_an_unproved_receiver_matches() {
    must_agree(
        "class Box\n  v: integer\n  fn init(v: integer)\n    self.v = v\n  end\nend\n\
         local b: any = Box(4)\n\
         local r: string = tostring(b?.v)\nr",
    );
    // And the nil arm still short-circuits rather than asking `read_member`
    // for a member of nil.
    must_agree(
        "local b: any? = nil\n\
         local r: string = tostring(b?.v)\nr",
    );
}

#[test]
fn a_safe_method_call_on_an_unproved_receiver_matches() {
    must_agree(
        "class Box\n\
         \x20 v: integer\n\
         \x20 fn init(v: integer)\n    self.v = v\n  end\n\
         \x20 fn doubled() -> integer\n    return self.v * 2\n  end\n\
         end\n\
         local b: any = Box(7)\n\
         local r: string = tostring(b?.doubled())\nr",
    );
    must_agree(
        "local b: any? = nil\n\
         local r: string = tostring(b?.doubled())\nr",
    );
}

#[test]
fn a_safe_call_on_an_unproved_receiver_does_not_evaluate_its_arguments() {
    // The nil guard wraps the *whole* call on the dynamic path too — the
    // tree-walker returns before evaluating arguments, so evaluating them
    // here would run side effects it does not. Counted rather than assumed.
    must_agree(
        "local calls: integer = 0\n\
         fn bump() -> integer\n  calls = calls + 1\n  return 1\nend\n\
         local b: any? = nil\n\
         local ignored: any? = b?.doubled(bump())\n\
         local r: string = \"\" .. calls\nr",
    );
}

#[test]
fn a_member_write_on_an_unproved_receiver_matches() {
    // `SETFX`, the write half of `GETFX`. It was `json_usage`'s first
    // refusal — a field reached through a value that came out of a decoder
    // as `any`.
    must_agree(
        "class Box\n  v: integer\n  fn init(v: integer)\n    self.v = v\n  end\nend\n\
         local b: any = Box(1)\n\
         b.v = 9\n\
         local r: string = \"\" .. (b as Box)!.v\nr",
    );
}

#[test]
fn a_member_write_on_an_unproved_receiver_reports_the_same_error() {
    // The error text has to match too, which is the whole reason this
    // defers to `assign_member` rather than reimplementing the write.
    must_agree(
        "class Box\n  v: integer\n  fn init(v: integer)\n    self.v = v\n  end\nend\n\
         local b: any = Box(1)\n\
         b.nope = 9\n\
         local r: string = \"done\"\nr",
    );
}


