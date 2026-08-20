//! Classes, interfaces, vtable inheritance, operator overloads, and statics (§8).

use crate::harness::*;

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


// ── interfaces ────────────────────────────────────────────────────────────

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


// ── interface method return types (front end) ─────────────────────────────

#[test]
fn an_interface_method_call_has_a_known_return_type() {
    // `saule-typeck` used to answer `cannot determine the type of this
    // expression` for **any** call on an interface-typed receiver, because
    // the semantic registry recorded only what each interface *extends* and
    // never its signatures. A single-valued method was as unusable as a
    // multi-valued one.
    must_agree(
        "interface Shape\n  fn area() -> integer\nend\n\
         class Square implements Shape\n\
         \x20 s: integer\n\
         \x20 fn init(s: integer)\n    self.s = s\n  end\n\
         \x20 fn area() -> integer\n    return self.s * self.s\n  end\n\
         end\n\
         fn describe(x: Shape) -> string\n\
         \x20 local a: integer = x.area()\n\
         \x20 return \"area \" .. a\n\
         end\n\
         describe(Square(5))",
    );
}

#[test]
fn an_interface_method_call_binds_two_results() {
    // Now reachable from source, which is what makes `CALLIF`'s packed
    // result count a live encoding rather than a speculative one.
    must_agree(
        "interface Splitter\n  fn halves() -> (integer, integer)\nend\n\
         class Seven implements Splitter\n\
         \x20 fn halves() -> (integer, integer)\n    return 3, 4\n  end\n\
         end\n\
         fn use(s: Splitter) -> string\n\
         \x20 local a: integer, b: integer = s.halves()\n\
         \x20 return a .. \"/\" .. b\n\
         end\n\
         use(Seven())",
    );
}

#[test]
fn an_extended_interfaces_method_is_found_through_the_extends_chain() {
    // An interface composes by extension rather than inheritance, so the
    // lookup walks a *list* per level. A method declared on the base has to
    // be reachable from the extending interface's static type.
    must_agree(
        "interface Named\n  fn name() -> string\nend\n\
         interface Shape extends Named\n  fn area() -> integer\nend\n\
         class Square implements Shape\n\
         \x20 s: integer\n\
         \x20 fn init(s: integer)\n    self.s = s\n  end\n\
         \x20 fn area() -> integer\n    return self.s * self.s\n  end\n\
         \x20 fn name() -> string\n    return \"square\"\n  end\n\
         end\n\
         fn describe(x: Shape) -> string\n\
         \x20 local n: string = x.name()\n\
         \x20 local a: integer = x.area()\n\
         \x20 return n .. \" \" .. a\n\
         end\n\
         describe(Square(3))",
    );
}


