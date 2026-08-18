# Saule VM — prompt for the next session

Paste everything below the line into a fresh chat.

---

I'm continuing work on the Saule bytecode VM.

## Read these first

- `VM_DESIGN.md` — the specification. Section references (§) throughout the
  code point into it. **Normative**: if the code and this disagree, the code
  is wrong unless the deviation is written down and argued.
- `VM_TASKS.md` — the LIVE checklist. What's done, what's deliberately
  deferred and why, measured numbers, every remaining gap, and the bugs
  found so far with the reasoning that found them. **Update it as you go.**

## Where things stand

Phases 0, 1, 2 complete. Phase 3 is nearly complete. Phase 4 (flip the
default) and Phase 5 (optimization) are untouched.

| Measure | State |
|---|---|
| `tests/*.sau` compiling fully | **85 of 92** |
| `examples/**/*.sau` compiling fully | **17 of 61** |
| `examples/*` projects running fully on the VM | **4 of 9 compared** (2 skipped: interactive) |
| `benchmarks/sau` | 10 of 10 |
| Differential tests | **167** |
| Release speedups | 2.1–3.5x on the microbenchmarks; 2.2–3.4x net of ~100ms startup on the real files |

Everything falls back cleanly, so the VM is safe on any program today. What
is missing costs speed, not correctness.

**Numbers in a handoff go stale.** The previous one said "84 of 92" and
"7 of 61"; both were already better at `HEAD` before this session started.
Re-measure before you plan around a number, and when you claim a change
moved one, verify it by censusing `HEAD` too — `git stash` and an
incremental rebuild is under a minute.

## Verify with all five — the last three are what catch VM bugs

```
RUST_MIN_STACK=16777216 cargo test --workspace          # fully green
SAULE_BIN=./target/debug/saule bash run_tests.sh        # 236/236
SAULE_ENGINE=vm SAULE_BIN=... bash run_tests.sh         # 236/236
SAULE_DIFF=1  SAULE_BIN=... bash run_tests.sh           # 236/236 + engines agree on OUTPUT
SAULE_BIN=... bash run_examples_diff.sh                 # 9/9 projects agree
```

Plus `cargo run --release -p saule-vm --example compare`, which asserts the
engines agree before timing.

**Platform notes.** On macOS the binary is `saule`, not `saule.exe`, and
`cargo` is on `PATH`. `RUST_MIN_STACK` is **required**: without it
`the_recursion_guard_still_unwinds_after_re_entrant_calls` overflows
libtest's 2 MiB thread and aborts the whole test binary — at `HEAD` too, so
check before blaming your change. `run_examples_diff.sh` used to need GNU
`timeout`, which stock macOS lacks; it now falls back to a shell watchdog.
On Windows, `cargo` is not on `PATH` — use
`C:\Users\lauri\.cargo\bin\cargo.exe`.

## What is left, in the order I would do it

### 1. The cross-module slice — 34 of the 44 remaining refusals

This is now the only thing of its size left, and it is what decides whether
the VM engages on code a user would write.

| Cause (first refusal per file) | Files |
|---|---|
| a class extending one the compiler cannot see | **24** |
| an import declaration | **10** |
| a name the resolver could not classify | 8 |
| a variant of an unknown enum | 1 |
| a class implementing `Assignable` | 1 |

The census command is in `VM_TASKS.md`. Note the `tr '\n' ' '` (miette wraps
long messages, and a line-oriented grep scores a refused file as compiling)
**and** use the `sed` form rather than `grep -o '`…`'` — several refusal
messages contain nested backticks, which the naive grep truncates into a
bogus separate cause. Both traps have produced wrong readings before.

First-refusal-wins, so closing one only reveals the next — measure again
after each. Expect the *file* count to move well before the *project* count
does; a project needs every one of its files to compile.

### 2. The remaining fixture long tail — 7, each independent

`valued_enum` (an enum with methods — needs §0.6's missing `NodeId` on enum
methods, or a different key), `match_tuple` (a tuple pattern), `io_lib` (a
prelude name outside a call), `compound_assign` (a compound assignment to a
member), `assignable` (a class implementing `Assignable`), `closure_capture`
(`self` outside a method), `module_variable` (a declaration the compiler
does not handle). Cheap individually; none unlocks another.

### 3. The open correctness items

- **Forward references from the module body — fixed, with one gap left.**
  Four divergences closed by three *exact* positional guards in the
  compiler: a `CALLK` to a `fn`, a `GETMOD` read, and a constructor of a
  class, each refused when the module body has not yet passed the
  declaration. They cost **zero** coverage — no file in either corpus is
  refused by any of them.

  The one still open is `a_forward_reference_reached_through_a_callee_still_diverges`:
  `C.go()` called above `fn later`, where `go`'s body references `later`.
  The reference itself is legal; only the call is early, and nothing local
  to the call site can tell. **A conservative "refuse any module-body call
  while a `fn` is still ahead" guard was tried and reverted** — it refused
  two perfectly good differential fixtures, because a call partway down a
  file with any `fn` below it is an ordinary shape. Closing it needs
  call-graph reachability, for a program the tree-walker rejects anyway.

- **The frame limit was a divergence and is now aligned.** §6.4 raised the
  VM's cap to 1,000,000 against the tree-walker's 10,000, on sound
  reasoning — a call here is a `Vec` push, not a native frame. But
  `depth(50_000)` then returned `50000` under `--vm` and raised
  `StackOverflow` without it. The cap is now the tree-walker's constant;
  the raise moves to Phase 4. This is why `TAILCALL` must also wait:
  implementing it would *create* a divergence, not close one.
- **Two refusals multi-return left behind**, both deliberate and both
  documented: `return a, f()` (the returned range must be contiguous, and
  the allocator cannot put `f`'s window immediately after a live `a`), and
  `return x?.m()` (the nil arm cannot set `top`). Neither is wrong, but both
  cost a fallback — and the second is now `todo-app`'s *first* refusal, so
  it has become the cheapest way to move a whole project.
- **`saule-typeck` cannot see through an interface method call's return
  type** — `local a: integer = s.half()` is `cannot determine the type of
  this expression` even for a single-valued method. A front-end gap, not a
  VM one, but it is why a parallel `local` from an interface call cannot be
  written and the `CALLIF` result-count encoding is only reachable through
  `return`.

### 4. Testing gaps — do these before Phase 4

- **Verifier tests.** Seven exist (in `compile/verify.rs`); a bad opcode
  byte, a stray `EXTRAARG`, and out-of-range `Bx` per table are not covered.
- `www/` in the differential harness (`examples/` is done).
- Closure-semantics fixtures asserting values; memory fixtures with recorded
  peak-RSS bounds; a benchmark regression gate with the ~3% noise floor.

### 5. Phase 4 — flip the default (1–2 weeks, mostly mechanical)

`--vm` becomes default, `--interp` opts out; `saule-wasm` switches; update
`PRODUCTION.md` with real numbers; confirm (don't assume) `saule-lsp` and
`saule-db` need no changes; keep the tree-walker in-tree — it is the
differential oracle.

**Do not start Phase 4 until real-program coverage is well above 4 of 9**,
or flipping the default just means most programs silently take the fallback.

### 6. Phase 5 — optimization, only with a profile in hand

Inline caches for `GETFX`/`CALLIF` (both now exist and are hash-probe
based), superinstructions from a measured histogram, precomputed hashes on
constant string keys, `get_unchecked` in the dispatch loop. `map` sits at
1.25x because it is hashing-bound inside `TableObject`, exactly as §20
predicts. A cheap one still unclaimed: `local x = f()` emits the call into a
fresh window and then a `MOVE` to `x`, on every call form — a peephole that
lands the window on the destination would delete it.

## Working conventions that have paid off — please keep them

- **Differential testing is the discipline.** `crates/saule-vm/tests/
  differential.rs` runs every program under both engines and compares
  results *including error text*. Add cases there first.
- **Refuse rather than guess.** Anything codegen cannot handle returns
  `CompileError::Unsupported` naming the construct, and the CLI falls back.
  A wrong slot reads different data and nothing notices.
- **Reuse rather than reimplement.** `ARITHX` calls `ops::binary`, `CASTCHK`
  calls `cast`, `GETFX` calls `read_member`, `CALLMX` calls
  `dispatch_member_call_multi`, `CONCAT` calls `display_value`, and
  `ITERPREPX` calls `call_member_dynamic` for an instance's `iter()`. Every
  time this rule was broken the engines diverged.
- **A missing type is never a wrong opcode** — it selects the dynamic form.
  The `X` suffix is that convention: `ARITHX`, `GETFX`, `CALLMX`,
  `ITERPREPX`.
- **Opcodes are appended, never inserted.** The numbering is the chunk ABI;
  `encoding.rs`'s `opcode_numbering_is_stable` pins the last one (now
  `ITERPREPX`) and must be updated with each addition. Operand *encodings*
  are part of that ABI too.
- Canaries to re-point when the named construct lands:
  `differential.rs`'s `unsupported_constructs_report_rather_than_miscompile`
  (currently `a tuple pattern`) and `handwritten.rs`'s unimplemented-opcode
  canary (currently `SUPER`).

## Five traps this codebase has already fallen into

1. **A module-level `local` is a module *slot*, not a frame local**, so
   `FuncCtx::lookup` structurally cannot see it. Three bugs came from checks
   built on that lookup alone. Use `Compiler::not_shadowed`, which checks
   both places, or the resolver's `Binding::Prelude` where that is the
   precise question.
2. **Widening what compiles can turn an inert documented gap into a live
   divergence.** `EnumObject.methods` was empty and *documented as safe*
   because enum-method calls refused — then `CALLMX` dispatched dynamically
   and reached the empty map. Re-read the "known safe because X refuses"
   notes whenever you delete a refusal.
3. **A reported refusal can be hiding an unreported miscompile.** Chasing
   `case x when x < 0` (a clean refusal) found a second bug beside it: a
   failed pattern jumped into the arm body whenever the arm had a guard —
   wrong value, exit status 0. Do not assume the mishandling is confined to
   the part that refused.
4. **Correct-by-accident survives exactly as long as nothing can observe
   it.** `return f()` truncated a callee's results to one, which was
   invisible while nothing compiled could consume two — and became a wrong
   value the moment a `for … in` driver could. Before widening coverage,
   ask what the *narrowness* was quietly making safe.
5. **Copying the oracle verbatim is wrong where the two engines represent a
   value differently.** `ITERPREPX`'s callable test was lifted from
   `exec_for_in`, which lists `Function`/`Native`/`NativeClosure` and never
   `VmFunction` — because the tree-walker cannot construct one. Under the VM
   every compiled closure *is* a `VmFunction`, so the first cut refused
   every driver on the new path. Reuse the oracle's *logic*; check its
   *type tests* against this engine's value representation.

## Uncommitted state

Everything is committed at `3b6c20a`. Uncommitted on top of it: the
`ITERPREPX` slice (`op.rs`, `vm/mod.rs`, `compile/stmt.rs`, `compile/ctx.rs`,
`disasm.rs`, `tests/differential.rs`, `tests/encoding.rs`), the
`run_examples_diff.sh` portability fix, and this file plus `VM_TASKS.md`.
All five verification commands pass as of this handoff.

Start with item 1 unless I say otherwise.
