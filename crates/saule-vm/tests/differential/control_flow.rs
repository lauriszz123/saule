//! `if`/`while`/numeric `for`, block scoping, short-circuiting, `break` and `continue`.

use crate::harness::*;

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


