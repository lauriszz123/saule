//! Compile-time argument binding: named, defaulted, and reordered arguments (§19).

use crate::harness::*;

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


