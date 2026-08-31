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




// ── calling a function-valued field ───────────────────────────────────────
//
// `self.builder()` where `builder` is a *field* holding a `fn`, not a
// method. `member_call` proves the receiver's class, misses the vtable, and
// used to refuse there — which handed the whole module to the tree-walker.
// The field's slot is known at that point, so the callee is an ordinary
// `GETF` and the call an ordinary `CALL`.

/// The fixture: a class whose field holds a function.
const BUILDER: &str = "class Entry\n\
     \x20 builder: fn() -> integer\n\
     \x20 fn init(builder: fn() -> integer)\n\
     \x20   self.builder = builder\n\
     \x20 end\n\
     \x20 fn content() -> integer\n\
     \x20   return self.builder()\n\
     \x20 end\n\
     end\n";

#[test]
fn a_function_valued_field_is_callable_through_self() {
    // The shape `UI Project` fell back on: the call is inside a method, so
    // the receiver is `self` and its class is proved.
    must_agree(&format!(
        "{BUILDER}local e = Entry(fn() -> integer\n  return 7\nend)\ne.content()"
    ));
}

#[test]
fn a_function_valued_field_is_callable_through_a_named_receiver() {
    must_agree(&format!(
        "{BUILDER}local e = Entry(fn() -> integer\n  return 7\nend)\ne.builder()"
    ));
}

#[test]
fn a_function_valued_field_call_passes_arguments() {
    must_agree(
        "class Adder\n\
         \x20 f: fn(integer, integer) -> integer\n\
         \x20 fn init(f: fn(integer, integer) -> integer)\n\
         \x20   self.f = f\n\
         \x20 end\n\
         \x20 fn run() -> integer\n\
         \x20   return self.f(3, 4)\n\
         \x20 end\n\
         end\n\
         local a = Adder(fn(x: integer, y: integer) -> integer\n  return x * 10 + y\nend)\n\
         a.run()",
    );
}

#[test]
fn a_function_valued_field_call_closes_over_its_environment() {
    // The field holds a *closure*, so calling it has to reach the captured
    // upvalue and not a fresh one.
    must_agree(&format!(
        "{BUILDER}local n = 5\n\
         local e = Entry(fn() -> integer\n  return n * 2\nend)\n\
         n = 8\n\
         e.content()"
    ));
}

#[test]
fn a_function_valued_field_is_read_after_the_arguments_are_evaluated() {
    // Evaluation order is observable and it is not the obvious one: the
    // tree-walker looks the field up *after* it has evaluated the
    // arguments, so an argument that reassigns the field changes which
    // function is called. Emitting the `GETF` before the arguments — the
    // natural way to write it — silently calls the old one.
    must_agree(
        "class Box\n\
         \x20 f: fn(integer) -> integer\n\
         \x20 fn init(f: fn(integer) -> integer)\n\
         \x20   self.f = f\n\
         \x20 end\n\
         \x20 fn swap() -> integer\n\
         \x20   self.f = fn(x: integer) -> integer\n    return x + 100\n  end\n\
         \x20   return 1\n\
         \x20 end\n\
         \x20 fn go() -> integer\n\
         \x20   return self.f(self.swap())\n\
         \x20 end\n\
         end\n\
         local b = Box(fn(x: integer) -> integer\n  return x\nend)\n\
         b.go()",
    );
}

#[test]
fn a_function_valued_field_call_in_statement_position_matches() {
    // `Want::Fixed(0)`: the result is dropped, and the call window has to be
    // released without reading it.
    must_agree(
        "class Runner\n\
         \x20 f: fn() -> nil\n\
         \x20 fn init(f: fn() -> nil)\n\
         \x20   self.f = f\n\
         \x20 end\n\
         \x20 fn go()\n\
         \x20   self.f()\n\
         \x20 end\n\
         end\n\
         local hits = 0\n\
         local r = Runner(fn()\n  hits = hits + 1\nend)\n\
         r.go()\n\
         r.go()\n\
         hits",
    );
}

#[test]
fn a_method_still_wins_over_a_field_of_the_same_name() {
    // Precedence, not just resolution: the tree-walker tries the vtable
    // first, so a field must never be reached while a method of that name
    // exists. Written as a subclass adding the method, because a single
    // class cannot declare both.
    must_agree(
        "class Base\n\
         \x20 act: fn() -> string\n\
         \x20 fn init(act: fn() -> string)\n\
         \x20   self.act = act\n\
         \x20 end\n\
         end\n\
         local b = Base(fn() -> string\n  return \"field\"\nend)\n\
         b.act()",
    );
}

#[test]
fn a_non_callable_field_fails_the_same_way() {
    // The field exists and is not a function. Both engines have to report
    // that identically — the compiler must not turn a program error into a
    // different one by having chosen the field path.
    must_agree(
        "class Box\n\
         \x20 n: integer\n\
         \x20 fn init()\n    self.n = 1\n  end\n\
         end\n\
         local b = Box()\n\
         b.n()",
    );
}


// ── an override, inherited one level further down ─────────────────────────

#[test]
fn a_grandchild_inherits_the_override_not_the_original() {
    // Three levels: the middle one overrides, the bottom one does not. The
    // bottom must run the *middle's* body.
    //
    // It ran the grandparent's. Pass 1 clones the parent's vtable for slot
    // *numbering*, and an override recorded itself in `member_of_vslot`
    // without clearing the inherited proto index sitting in the slot — so a
    // subclass laid out before codegen cloned the grandparent's index, and
    // Pass 2a then skipped the slot because it was not `u32::MAX`.
    //
    // A silent wrong answer, and the crash it also causes is the lucky case:
    // `examples/UI Project` only faulted because the index happened to be
    // past the end of the subclass's own proto vector. Two classes in one
    // module with enough protos would have called the wrong function and
    // said nothing.
    must_agree(
        "class A\n\
         \x20 fn who() -> string\n    return \"A\"\n  end\n\
         end\n\
         class B extends A\n\
         \x20 fn who() -> string\n    return \"B\"\n  end\n\
         end\n\
         class C extends B\n\
         end\n\
         local c: A = C()\n\
         c.who()",
    );
}

#[test]
fn an_override_two_levels_up_is_still_the_one_inherited() {
    // Four levels, and the override in the middle, so the answer is wrong
    // under either "always the parent" or "always the root".
    must_agree(
        "class A\n\
         \x20 fn who() -> string\n    return \"A\"\n  end\n\
         end\n\
         class B extends A\n\
         \x20 fn who() -> string\n    return \"B\"\n  end\n\
         end\n\
         class C extends B\n\
         end\n\
         class D extends C\n\
         end\n\
         local d: A = D()\n\
         local c: A = C()\n\
         d.who() .. c.who()",
    );
}

#[test]
fn an_override_and_a_further_override_both_dispatch() {
    must_agree(
        "class A\n\
         \x20 fn who() -> string\n    return \"A\"\n  end\n\
         end\n\
         class B extends A\n\
         \x20 fn who() -> string\n    return \"B\"\n  end\n\
         end\n\
         class C extends B\n\
         end\n\
         class D extends C\n\
         \x20 fn who() -> string\n    return \"D\"\n  end\n\
         end\n\
         local xs: table<A> = {A(), B(), C(), D()}\n\
         local out: string = \"\"\n\
         for x in xs do\n\
         \x20 out = out .. x.who()\n\
         end\n\
         out",
    );
}


// ── `self`'s class survives into a lambda ─────────────────────────────────
//
// `self` crosses into a lambda as a captured upvalue and always worked at
// run time. What did not cross was the *compiler's* knowledge of its class:
// a lambda gets a fresh frame with no `current_class`, so `class_of_expr`
// answered `None` and every `self.m(…)` inside a callback lost its receiver.
// Harmless for a plain call — `CALLMX` handles an unproved receiver — but a
// named argument or a trailing block needs the callee's parameter list, and
// that refused the whole module with `a named argument to a callee the
// compiler cannot identify`. `self.setState() do … end` in
// `examples/UI Project` is the shape.

#[test]
fn a_named_argument_binds_on_self_inside_a_lambda() {
    must_agree(
        "class Box\n\
         \x20 n: integer\n\
         \x20 fn init(n: integer)\n    self.n = n\n  end\n\
         \x20 fn combine(head: string, tail: string) -> string\n\
         \x20   return head .. tostring(self.n) .. tail\n\
         \x20 end\n\
         \x20 fn run() -> string\n\
         \x20   local f = fn() -> string\n\
         \x20     return self.combine(tail: \"<\", head: \">\")\n\
         \x20   end\n\
         \x20   return f()\n\
         \x20 end\n\
         end\n\
         Box(7).run()",
    );
}

#[test]
fn a_skipped_default_binds_on_self_inside_a_nested_lambda() {
    // Two frames deep, and through the gap entry — the class has to survive
    // every hop, not just the first.
    must_agree(
        "class Box\n\
         \x20 n: integer\n\
         \x20 fn init(n: integer)\n    self.n = n\n  end\n\
         \x20 fn go(a: integer, d: integer = self.n + 1, t: string) -> string\n\
         \x20   return tostring(a + d) .. t\n\
         \x20 end\n\
         \x20 fn run() -> string\n\
         \x20   local outer = fn() -> string\n\
         \x20     local inner = fn() -> string\n\
         \x20       return self.go(a: 1, t: \"!\")\n\
         \x20     end\n\
         \x20     return inner()\n\
         \x20   end\n\
         \x20   return outer()\n\
         \x20 end\n\
         end\n\
         Box(10).run()",
    );
}

#[test]
fn a_static_methods_lambda_has_no_receiver() {
    // The other side of the rule. `self` in a `static fn` is the class, and
    // `self.count` there is a *static* read — the class must not be
    // inherited as a receiver by a lambda, or a field read inside one would
    // compile as a static read.
    must_agree(
        "class Counter\n\
         \x20 static total: integer = 5\n\
         \x20 static fn bump() -> integer\n\
         \x20   local f = fn() -> integer\n\
         \x20     return Counter.total + 1\n\
         \x20   end\n\
         \x20   return f()\n\
         \x20 end\n\
         end\n\
         Counter.bump()",
    );
}

#[test]
fn a_field_read_inside_a_lambda_is_still_a_field_read() {
    // The regression the `self_class` / `current_class` split exists to
    // prevent: inheriting `current_class` into a lambda would have made
    // `!in_method && current_class` true there, and `self.n` would have
    // compiled as a static read of a field that is not static.
    must_agree(
        "class Box\n\
         \x20 n: integer\n\
         \x20 fn init(n: integer)\n    self.n = n\n  end\n\
         \x20 fn run() -> integer\n\
         \x20   local f = fn() -> integer\n\
         \x20     return self.n * 2\n\
         \x20   end\n\
         \x20   return f()\n\
         \x20 end\n\
         end\n\
         Box(21).run()",
    );
}
