//! `try`/`catch`/`throw` (§12).

use crate::harness::*;

// ── try / catch / throw ───────────────────────────────────────────────────

#[test]
fn a_caught_throw_matches() {
    must_agree(
        "local r: string = \"none\"\n\
         try\n  throw \"boom\"\ncatch e: string\n  r = e\nend\nr",
    );
}

#[test]
fn a_try_that_does_not_throw_runs_no_handler() {
    // The happy path emits no instructions for the `try` at all.
    must_agree(
        "local r: integer = 0\n\
         try\n  r = 1\ncatch e: string\n  r = 2\nend\nr",
    );
}

#[test]
fn a_throw_from_a_called_function_unwinds_to_the_caller() {
    must_agree(
        "fn boom() -> nil\n  throw \"deep\"\nend\n\
         local r: string = \"none\"\n\
         try\n  boom()\ncatch e: string\n  r = e\nend\nr",
    );
}

#[test]
fn an_uncaught_throw_reports_the_same_way() {
    must_agree("throw \"escaped\"");
    must_agree("fn f() -> nil\n  throw \"from f\"\nend\nf()");
}

#[test]
fn a_catch_of_the_wrong_type_does_not_catch() {
    must_agree(
        "local r: integer = 0\n\
         try\n  throw \"a string\"\ncatch e: integer\n  r = 1\nend\nr",
    );
}

#[test]
fn nested_try_catches_at_the_inner_handler() {
    must_agree(
        "local r: string = \"none\"\n\
         try\n\
         \x20 try\n    throw \"inner\"\n  catch e: string\n    r = \"caught \" .. e\n  end\n\
         catch e2: string\n  r = \"outer\"\nend\nr",
    );
}

#[test]
fn a_loop_inside_a_try_still_works() {
    must_agree(
        "local s: integer = 0\n\
         try\n  for i = 1, 5 do s = s + i end\ncatch e: string\n  s = -1\nend\ns",
    );
}


#[test]
fn a_nullable_catch_type_does_not_catch_everything() {
    // A live silent divergence, not a gap: `TypeDesc` had no `Nullable`, so
    // `catch e: string?` interned as `Any` and caught a thrown integer the
    // tree-walker lets escape. `runtime_matches_type` reads `T?` as
    // `nil || T`, and `TypeDesc::Nullable` now does the same.
    must_agree(
        "local caught: string = \"no\"\n         try\n           try\n             throw 42\n           catch e: string?\n             caught = \"yes\"\n           end\n         catch outer: any\n           caught = \"escaped\"\n         end\n         caught",
    );
    // ...and still catches what it should, on both sides of the `?`.
    must_agree(
        "local out: string = \"\"\n         try\n           throw \"boom\"\n         catch e: string?\n           out = e ?? \"nil\"\n         end\n         try\n           throw nil\n         catch e: string?\n           out = out .. \"|\" .. (e ?? \"nil\")\n         end\n         out",
    );
}


