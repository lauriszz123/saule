//! Table construction and indexing, `repeat`, compound assignment, stdlib constants.

use crate::harness::*;

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


#[test]
fn a_prelude_name_in_a_value_position_folds() {
    // `Io.stdout` is an object, not one of the scalars `prelude_member`
    // folds, so the bare `Io` had to become a value of its own. The prelude
    // is fixed before a program runs, so it is one `LOADK`.
    must_agree("tostring(type(Io)) .. tostring(type(Math))");
}

#[test]
fn a_shadowed_prelude_name_in_a_value_position_is_not_folded() {
    // Trap 1 again: a module-level `local` is a module *slot*, so
    // `FuncCtx::lookup` cannot see it and only `static_value`'s
    // `not_shadowed` gate keeps the program's own table from becoming the
    // stdlib's.
    must_agree(
        "local Math: table<string, float> = {pi: 3.0}\n         local m: table<string, float> = Math\n         m[\"pi\"]",
    );
}


