//! Enums, `match`, guards, tuple patterns, and nested payload patterns (§9).

use crate::harness::*;

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


// ── tuple patterns and nested payload patterns ────────────────────────────

#[test]
fn a_tuple_pattern_destructures_a_multi_return_scrutinee() {
    // The shape the fixture uses: a call returning two values, matched
    // positionally, with a literal in one position.
    must_agree(
        "fn divmod(a: integer, b: integer) -> (integer, integer)\n\
         \x20 return a / b, a % b\n\
         end\n\
         fn describe(a: integer, b: integer) -> string\n\
         \x20 return match divmod(a, b)\n\
         \x20   case (q, 0) then \"clean \" .. q\n\
         \x20   case (q, r) then q .. \" rem \" .. r\n\
         \x20 end\n\
         end\n\
         describe(10, 2) .. \"/\" .. describe(10, 3)",
    );
}

#[test]
fn a_tuple_pattern_wider_than_the_scrutinee_does_not_match() {
    // The oracle's rule is `values.len() < elems.len()` fails, and it is
    // reachable in a well-typed program — the typechecker allows
    // `case (a, b, c)` over a two-value call.
    //
    // **This is why `NVALS` exists.** A compiler that evaluated the
    // scrutinee into a fixed window and padded with nil would have no way to
    // tell "returned nil" from "returned nothing", would match the
    // three-element arm, and would answer `three` where the tree-walker
    // answers `two`.
    must_agree(
        "fn two() -> (integer, integer)\n\
         \x20 return 1, 2\n\
         end\n\
         local s: string = match two()\n\
         \x20 case (a, b, c) then \"three\"\n\
         \x20 case (a, b) then \"two\"\n\
         \x20 case _ then \"other\"\n\
         end\ns",
    );
}

#[test]
fn a_tuple_patterns_second_element_survives_the_count() {
    // The regression that took the longest to see. `NVALS` writes the value
    // count into a register, and with `Want::All` the results can extend
    // *above* the window the allocator sized for the call's arguments — so
    // the first cut allocated that register on top of `values[1]` and bound
    // `r` to the count.
    //
    // `return 4, 0` and a literal `0` in the second position is the shape
    // that catches it: the count is 2, so a clobbered `values[1]` reads as
    // 2, the `case (q, 0)` arm fails, and the answer silently becomes
    // `4 rem 2` instead of `clean 4`. Exit status 0 either way.
    must_agree(
        "fn two() -> (integer, integer)\n\
         \x20 return 4, 0\n\
         end\n\
         local s: string = match two()\n\
         \x20 case (q, 0) then \"clean \" .. q\n\
         \x20 case (q, r) then q .. \" rem \" .. r\n\
         end\ns",
    );
}

#[test]
fn a_tuple_pattern_over_a_non_call_scrutinee() {
    // `eval_values` on anything that is not a call yields exactly one value,
    // so the length test is decidable at compile time and no `NVALS` is
    // emitted. Both directions are asserted: the one-element pattern
    // matches, and the wider one cannot.
    must_agree(
        "local n: integer = 9\n\
         local a: string = match n\n\
         \x20 case (x) then \"one:\" .. x\n\
         \x20 case _ then \"other\"\n\
         end\n\
         local b: string = match n\n\
         \x20 case (x, y) then \"two\"\n\
         \x20 case _ then \"other\"\n\
         end\n\
         a .. \"/\" .. b",
    );
}

#[test]
fn a_literal_inside_a_variant_payload_selects_the_arm() {
    // Nested patterns in a payload used to refuse outright (`a nested
    // pattern in a variant payload`), which is what kept the jump-table path
    // safe without knowing it: `switchable` accepted any variant arm, and a
    // payload sub-pattern that can *fail* has no next arm to jump to there.
    //
    // Trap 2 exactly — an inert gap that widening turns into a live
    // divergence — so `switchable` now requires every payload sub-pattern to
    // be irrefutable, and a refutable one takes the chain. This asserts the
    // answers, and `both_arms_of_a_refutable_payload_still_switch` below
    // asserts the fast path did not simply stop being taken.
    must_agree(
        "enum Shape\n\
         \x20 Circle(r: integer),\n\
         \x20 Rect(w: integer, h: integer),\n\
         \x20 Dot\n\
         end\n\
         fn describe(s: Shape) -> string\n\
         \x20 return match s\n\
         \x20   case Shape.Circle(0) then \"unit\"\n\
         \x20   case Shape.Circle(r) then \"circle \" .. r\n\
         \x20   case Shape.Rect(2, h) then \"narrow \" .. h\n\
         \x20   case Shape.Rect(w, h) then \"rect \" .. w .. \"x\" .. h\n\
         \x20   case _ then \"dot\"\n\
         \x20 end\n\
         end\n\
         describe(Shape.Circle(0)) .. \"|\" .. describe(Shape.Circle(7))\n\
         \x20 .. \"|\" .. describe(Shape.Rect(2, 3)) .. \"|\" .. describe(Shape.Rect(5, 3))\n\
         \x20 .. \"|\" .. describe(Shape.Dot)",
    );
}

#[test]
fn a_wildcard_inside_a_variant_payload_still_switches() {
    // The other side of the `switchable` guard: a wildcard and a bind are
    // both irrefutable, so this arm set must keep the jump table. Asserted
    // on the disassembly, because the answers alone cannot tell which path
    // ran — and a guard that quietly sent every payload arm to the chain
    // would be a silent performance regression rather than a wrong answer.
    let src = "enum Shape\n\
               \x20 Circle(r: integer),\n\
               \x20 Rect(w: integer, h: integer),\n\
               \x20 Dot\n\
               end\n\
               fn describe(s: Shape) -> string\n\
               \x20 return match s\n\
               \x20   case Shape.Circle(_) then \"circle\"\n\
               \x20   case Shape.Rect(w, _) then \"rect \" .. w\n\
               \x20   case Shape.Dot then \"dot\"\n\
               \x20 end\n\
               end\n\
               describe(Shape.Rect(5, 3))";
    must_agree(src);
    let module = front_end(src);
    let chunk = saule_vm::compile(&module, "x.sau", src).expect("compiles");
    let text = saule_vm::disasm::chunk(&chunk).to_string();
    assert!(
        text.contains("SWITCH"),
        "an irrefutable payload must still reach the jump table:\n{text}"
    );
}

#[test]
fn a_prelude_enums_variants_match_by_tag() {
    // `OsPlatform` is defined in Rust, not in any Saule module, so it is in
    // no layout table and matching on it used to refuse as `a variant of an
    // unknown enum` — which is what sent `examples/fs-info-example` to the
    // tree-walker. Its tags are dense and fixed before the program runs, so
    // the compiler can read them from the prelude for the same reason it
    // folds `Math.pi`.
    must_agree(
        "local p: OsPlatform = Os.platform()\n\
         local s: string = match p\n\
         \x20 case OsPlatform.Linux then \"linux\"\n\
         \x20 case OsPlatform.Macos then \"macos\"\n\
         \x20 case OsPlatform.Windows then \"windows\"\n\
         \x20 case _ then \"other\"\n\
         end\n\
         s == \"\"",
    );
}

#[test]
fn a_shadowed_prelude_enum_is_not_read_from_the_prelude() {
    // Trap 1, in the place the prelude-enum lookup opens up. A module-level
    // `local OsPlatform = {…}` is a module *slot*, and the compiler must not
    // answer the pattern from the stdlib's enum of the same name.
    //
    // `must_agree` is the assertion either way: whatever the tree-walker
    // makes of this, the VM has to make the same thing of it — and folding
    // the prelude's tags here would produce a different answer rather than a
    // refusal.
    must_agree(
        "local OsPlatform = {Linux: 1}\n\
         local v: integer = OsPlatform.Linux\n\
         v",
    );
}


#[test]
fn an_enum_method_runs_on_the_variant_that_received_it() {
    // The refusal was structural: `EnumObject::methods` could only hold a
    // tree-walker `FunctionObject`, so a VM-built enum had an empty map and
    // `CALLMX` would have reported `no property or method` where the
    // tree-walker succeeds. `MethodRef` is what makes both representable,
    // exactly as it already did for classes.
    must_agree(
        "enum Status\n           Alive = \"alive\",\n           Dead = \"dead\"\n           fn describe() -> string\n             return \"Status is: \" .. self.value\n           end\n         end\n         local s: Status = Status.Alive\n         s.describe() .. \"/\" .. Status.Dead.describe() .. \"/\" .. Status.Dead.name",
    );
}


