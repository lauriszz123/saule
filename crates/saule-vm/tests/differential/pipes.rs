//! The pipe operator.

use crate::harness::*;

// ── pipes ─────────────────────────────────────────────────────────────────
//
// `when(source):a(x):b(y)` lowers to a chain of ordinary calls, each
// threading the upstream value in as argument 0 — what `eval`'s `Expr::Pipe`
// arm does. The value lives in one register for the whole chain.
//
// The callee is resolved **by name**: a `PipeStage` holds a `String` and has
// no `NodeId`, so the binding table has nothing keyed on it and the lookup
// order is written out by hand. These pin that the hand-written order agrees
// with the resolver's.

#[test]
fn a_pipeline_threads_its_value_through_each_stage() {
    must_agree(
        "fn double(n: integer) -> integer\n  return n * 2\nend\n\
         local r: integer = when(4):double()\nr",
    );
    // Chained, so a stage reading a stale register would show up.
    must_agree(
        "fn double(n: integer) -> integer\n  return n * 2\nend\n\
         local r: integer = when(3):double():double():double()\nr",
    );
}

#[test]
fn a_pipeline_stage_takes_extra_arguments_after_the_piped_value() {
    // The piped value is argument 0 and the written ones follow, so an
    // off-by-one in the window would swap `a` and `b` here — and `add` is
    // commutative on purpose *not* chosen: `sub` would hide nothing.
    must_agree(
        "fn sub(a: integer, b: integer) -> integer\n  return a - b\nend\n\
         local r: integer = when(10):sub(3)\nr",
    );
    must_agree(
        "fn sub(a: integer, b: integer) -> integer\n  return a - b\nend\n\
         fn double(n: integer) -> integer\n  return n * 2\nend\n\
         local r: integer = when(10):sub(3):double():sub(1)\nr",
    );
}

// **Not** covered here, deliberately:
//
// * a *prelude* name as a stage — `saule-typeck` rejects
//   `when(x):tostring()` with `UnknownPipeStage`, so it never reaches a
//   valid program and the compiler has no branch for it;
// * a stage naming a `fn` declared *below* the pipeline at module level.
//   The two engines genuinely disagree there — and they disagree about a
//   plain `later(5)` written the same way, with no pipe involved, so it
//   predates this work. Written up in VM_TASKS.md rather than papered over
//   with a skipped test.

#[test]
fn a_pipeline_over_a_table_matches() {
    must_agree(
        "fn total(xs: table<integer>) -> integer\n\
         \x20 local s: integer = 0\n\
         \x20 for v in xs do\n\
         \x20   s = s + v\n\
         \x20 end\n\
         \x20 return s\n\
         end\n\
         local t: table<integer> = {5, 1, 4}\n\
         local r: integer = when(t):total()\nr",
    );
}



