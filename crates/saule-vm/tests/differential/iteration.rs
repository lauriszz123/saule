//! `for … in`: proved sources, closure drivers, and unproved sources (§15.8).

use crate::harness::*;

// ── for … in ──────────────────────────────────────────────────────────────

#[test]
fn iterating_an_array_matches() {
    must_agree("local t: table<integer> = {10, 20, 30}\nlocal s: integer = 0\nfor v in t do s = s + v end\ns");
    must_agree(
        "local t: table<integer> = {10, 20, 30}\nlocal s: integer = 0\n\
         for i, v in t do s = s + i * v end\ns",
    );
}

#[test]
fn iterating_an_empty_table_runs_no_iterations() {
    must_agree("local t: table<integer> = {}\nlocal s: integer = 99\nfor v in t do s = 0 end\ns");
}

#[test]
fn break_and_continue_work_in_for_in() {
    must_agree(
        "local t: table<integer> = {1, 2, 3, 4, 5}\nlocal s: integer = 0\n\
         for v in t do\n  if v > 3 then break end\n  s = s + v\nend\ns",
    );
    must_agree(
        "local t: table<integer> = {1, 2, 3, 4, 5}\nlocal s: integer = 0\n\
         for v in t do\n  if v % 2 == 0 then continue end\n  s = s + v\nend\ns",
    );
}

#[test]
fn nested_for_in_matches() {
    must_agree(
        "local t: table<integer> = {1, 2, 3}\nlocal s: integer = 0\n\
         for a in t do\n  for b in t do\n    s = s + a * b\n  end\nend\ns",
    );
}


// ── `for … in` over a closure driver (§15.8) ──────────────────────────────
//
// Lowered to an ordinary `CALL` in a `while` shape rather than taught to
// `ITERNEXT`: `CALL` already dispatches on whatever it finds — a bytecode
// closure, a native, a native closure — so the driver can be any of them
// with no new opcode. The result count is fixed at `nvars`, which is what
// makes "extras → nil, surplus dropped" fall out of `pop_frame` instead of
// needing its own rule.

#[test]
fn a_closure_drives_a_for_in_loop() {
    must_agree(
        "fn counter(stop: integer) -> fn() -> integer?\n\
         \x20 local i: integer = 0\n\
         \x20 return fn()\n\
         \x20   i = i + 1\n\
         \x20   if i > stop then return nil end\n\
         \x20   return i\n\
         \x20 end\n\
         end\n\
         local sum: integer = 0\n\
         for n in counter(4) do\n\
         \x20 sum = sum + n\n\
         end\n\
         sum",
    );
}

#[test]
fn a_driver_that_yields_nothing_runs_no_iterations() {
    // The case that decided the calling convention. Asking for *all*
    // results would leave the callee register holding the driver itself
    // when a step returned nothing — a function, not nil — and the loop
    // would never end. A fixed result count pads with nil instead.
    must_agree(
        "fn empty() -> fn() -> integer?\n\
         \x20 return fn()\n\
         \x20   return nil\n\
         \x20 end\n\
         end\n\
         local n: integer = 0\n\
         for x in empty() do\n\
         \x20 n = n + 1\n\
         end\n\
         n",
    );
}

#[test]
fn break_and_continue_work_inside_a_driver_loop() {
    // `continue` re-enters at the *call* — the next step is what advances
    // this loop, so there is no separate increment to jump to.
    must_agree(
        "fn counter(stop: integer) -> fn() -> integer?\n\
         \x20 local i: integer = 0\n\
         \x20 return fn()\n\
         \x20   i = i + 1\n\
         \x20   if i > stop then return nil end\n\
         \x20   return i\n\
         \x20 end\n\
         end\n\
         local sum: integer = 0\n\
         for n in counter(10) do\n\
         \x20 if n == 3 then continue end\n\
         \x20 if n == 6 then break end\n\
         \x20 sum = sum + n\n\
         end\n\
         sum",
    );
}

#[test]
fn driver_loops_nest() {
    // Each loop holds its driver in a register of its own; sharing one
    // would make the inner loop exhaust the outer one's.
    must_agree(
        "fn counter(stop: integer) -> fn() -> integer?\n\
         \x20 local i: integer = 0\n\
         \x20 return fn()\n\
         \x20   i = i + 1\n\
         \x20   if i > stop then return nil end\n\
         \x20   return i\n\
         \x20 end\n\
         end\n\
         local out: string = \"\"\n\
         for a in counter(2) do\n\
         \x20 for b in counter(2) do\n\
         \x20   out = out .. a .. b .. \" \"\n\
         \x20 end\n\
         end\n\
         out",
    );
}


// ── §15.8 iteration over an unproved source ───────────────────────────────
//
// The dynamic `for … in` path (`ITERPREPX`). Every case launders its source
// through `any` so the front end cannot prove it — the shape real code hits,
// where a `Json.decode` result is iterated behind a `type(x) == "table"`
// guard that no static type can see through.

/// Prefix a program with a launderer that erases its source's static type.
fn unproved(body: &str) -> String {
    format!("fn opaque(v: any) -> any\n  return v\nend\n{body}")
}

#[test]
fn an_unproved_table_source_iterates() {
    must_agree(&unproved(
        "local t: table<any> = {10, 20, 30}
         local n: integer = 0
         for v in opaque(t) do
           n = n + 1
         end
         n",
    ));
}

#[test]
fn a_table_holding_a_nil_iterates_past_it_under_an_unproved_source() {
    // **The trap that decided `ITERPREPX`'s design.** The tempting lowering
    // normalises every source into one nil-terminated driver, so a single
    // opcode serves table, function and instance alike. It cannot work: a
    // driver stops on a nil and a table snapshot has no terminator at all,
    // and Saule's `t[i] = nil` *stores* a nil rather than deleting the key
    // (unlike Lua), so a table really can hold one. A one-variable loop
    // binds the **value** — so a normalising driver stops this at 1 where
    // the tree-walker runs it to 3.
    //
    // Nothing about that failure is loud: right exit status, wrong number.
    // Hence the mode flag and the two emitted steps.
    must_agree(&unproved(
        "local t: table<any> = {1, 2, 3}
         t[2] = nil
         local n: integer = 0
         for v in opaque(t) do
           n = n + 1
         end
         n",
    ));
}

#[test]
fn a_leading_nil_does_not_end_an_unproved_iteration() {
    // The same trap at its starkest: a normalising driver yields *zero*
    // iterations here, because the first value it sees is its terminator.
    must_agree(&unproved(
        "local t: table<any> = {1, 2, 3}
         t[1] = nil
         local n: integer = 0
         for v in opaque(t) do
           n = n + 1
         end
         n",
    ));
}

#[test]
fn an_unproved_two_variable_loop_binds_key_then_value() {
    // Also pins the register placement: with two variables the driver's
    // call window sits on `R[A+3]`, exactly where `ITERNEXT` writes the key.
    must_agree(&unproved(
        "local t: table<any> = {7, 8, 9}
         local out: string = \"\"
         for k, v in opaque(t) do
           out = out .. k .. \":\" .. v .. \" \"
         end
         out",
    ));
}

#[test]
fn an_unproved_map_source_yields_sorted_entries() {
    // The snapshot's ordering is observable, so the dynamic path has to
    // reuse `snapshot_pairs` rather than walk the map directly.
    must_agree(&unproved(
        "local m: table<string, any> = {}
         m[\"b\"] = 2
         m[\"a\"] = 1
         m[\"c\"] = 3
         local out: string = \"\"
         for k, v in opaque(m) do
           out = out .. k .. tostring(v)
         end
         out",
    ));
}

#[test]
fn an_unproved_driver_source_iterates() {
    // A compiled closure arrives as `Value::VmFunction`, which the
    // tree-walker's own callable test does not list — it never constructs
    // one. Omitting it here refused every driver under this path.
    must_agree(&unproved(
        "fn counter(stop: integer) -> fn() -> integer?
           local i: integer = 0
           return fn()
             i = i + 1
             if i > stop then return nil end
             return i
           end
         end
         local sum: integer = 0
         for n in opaque(counter(4)) do
           sum = sum + n
         end
         sum",
    ));
}

#[test]
fn an_unproved_instance_source_calls_iter() {
    must_agree(&unproved(
        "class Range implements Iterable
           local lo: integer
           local hi: integer
           fn init(lo: integer, hi: integer)
             self.lo = lo
             self.hi = hi
           end
           fn iter() -> fn() -> integer?
             local i: integer = self.lo - 1
             local stop: integer = self.hi
             return fn()
               i = i + 1
               if i > stop then return nil end
               return i
             end
           end
         end
         local sum: integer = 0
         for n in opaque(Range(1, 4)) do
           sum = sum + n
         end
         sum",
    ));
}

#[test]
fn an_unproved_empty_table_runs_no_iterations() {
    // `ITERPREPX`'s `Bx` jump — the only forward displacement it carries,
    // and taken only for a table, never for a driver, which must be called
    // at least once before it can say stop.
    must_agree(&unproved(
        "local t: table<any> = {}
         local n: integer = 0
         for v in opaque(t) do
           n = n + 1
         end
         n",
    ));
}

#[test]
fn break_and_continue_work_in_an_unproved_for_in() {
    // Both modes share one body, so `break` and `continue` have to land on
    // the merged step and the merged exit rather than on either path's own.
    must_agree(&unproved(
        "local t: table<any> = {1, 2, 3, 4, 5}
         local seen: integer = 0
         local kept: integer = 0
         for v in opaque(t) do
           seen = seen + 1
           if seen == 2 then continue end
           if seen == 4 then break end
           kept = kept + 1
         end
         seen * 10 + kept",
    ));
}

#[test]
fn a_nested_unproved_for_in_matches() {
    // Two live control blocks at once, one of each mode.
    must_agree(&unproved(
        "fn twice() -> fn() -> integer?
           local i: integer = 0
           return fn()
             i = i + 1
             if i > 2 then return nil end
             return i
           end
         end
         local t: table<any> = {1, 2, 3}
         local n: integer = 0
         for a in opaque(t) do
           for b in opaque(twice()) do
             n = n + 1
           end
         end
         n",
    ));
}

#[test]
fn a_non_iterable_unproved_source_reports_the_same_error() {
    // The compiler can no longer refuse this, so the *runtime* message is
    // now the contract — and it is the tree-walker's, not `ITERPREP`'s
    // narrower "needs a table".
    must_agree(&unproved(
        "local n: integer = 0
         for v in opaque(42) do
           n = n + 1
         end
         n",
    ));
}

#[test]
fn an_unproved_iter_returning_a_non_function_reports_the_same_error() {
    must_agree(&unproved(
        "class BadIter
           fn iter() -> any
             return 5
           end
         end
         local n: integer = 0
         for v in opaque(BadIter()) do
           n = n + 1
         end
         n",
    ));
}


