//! Literals, module-level locals, arithmetic, comparison, and string concatenation.

use crate::harness::*;

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


