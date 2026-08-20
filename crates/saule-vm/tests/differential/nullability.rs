//! Nullability: `?.`, `??`, `!`, and `as` (§15.12).

use crate::harness::*;

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


// ── `return x?.m()` and `return a, f()` ───────────────────────────────────

#[test]
fn a_returned_safe_call_passes_every_result_through() {
    // The two arms return **separately** — a nil receiver yields one nil,
    // a present one yields everything the method produced — which is why
    // this no longer needs a single register run for one `RET` to read.
    must_agree(
        "class Box\n\
         \x20 v: integer?\n\
         \x20 fn init(v: integer?)\n    self.v = v\n  end\n\
         \x20 fn twin() -> (integer?, integer?)\n    return self.v, self.v\n  end\n\
         end\n\
         fn twoOf(b: Box?) -> (integer?, integer?)\n\
         \x20 return b?.twin()\n\
         end\n\
         local p: integer?, q: integer? = twoOf(Box(5))\n\
         local r: integer?, s: integer? = twoOf(nil)\n\
         local out: string = tostring(p) .. tostring(q) .. tostring(r) .. tostring(s)\nout",
    );
}

#[test]
fn a_returned_safe_call_still_yields_one_value_where_it_should() {
    must_agree(
        "class Box\n\
         \x20 v: integer?\n\
         \x20 fn init(v: integer?)\n    self.v = v\n  end\n\
         \x20 fn one() -> integer?\n    return self.v\n  end\n\
         end\n\
         fn oneOf(b: Box?) -> integer?\n  return b?.one()\nend\n\
         local a: integer? = oneOf(Box(9))\n\
         local b: integer? = oneOf(nil)\n\
         local r: string = tostring(a) .. \"/\" .. tostring(b)\nr",
    );
}


