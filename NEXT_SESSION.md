# Saule VM — prompt for the next session

Paste everything below the line into a fresh chat.

---

I'm continuing work on the Saule bytecode VM in
`C:\Users\lauri\Documents\Codai\rust\saule`.

## Read these first

- `VM_DESIGN.md` — the specification. Section references (§) throughout the
  code point into it. **Normative**: if the code and this disagree, the code
  is wrong unless the deviation is written down and argued.
- `VM_TASKS.md` — the LIVE checklist. What's done, what's deliberately
  deferred and why, measured numbers, every remaining gap, and the bugs
  found so far with the reasoning that found them. **Update it as you go.**

## Where things stand

Phases 0, 1, 2 complete. Phase 3 is nearly complete — every language feature
on §21.4's list has landed, including multi-return. Phase 4 (flip the
default) and Phase 5 (optimization) are untouched.

| Measure | State |
|---|---|
| `tests/*.sau` compiling fully | **84 of 92** |
| `examples/*` projects running fully on the VM | **4 of 11** |
| `benchmarks/sau` | 10 of 10 |
| Differential tests | 150 |
| Release speedups | 2.1–3.5x on the microbenchmarks; 2.2–3.4x net of ~100ms startup on the real files |

Everything falls back cleanly, so the VM is safe on any program today. What
is missing costs speed, not correctness.

## Verify with all five — the last three are what catch VM bugs

```
cargo test --workspace                                       # 62 binaries, fully green
SAULE_BIN=./target/debug/saule.exe bash run_tests.sh         # 236/236
SAULE_ENGINE=vm SAULE_BIN=... bash run_tests.sh              # 236/236
SAULE_DIFF=1  SAULE_BIN=... bash run_tests.sh                # 236/236 + engines agree on OUTPUT
SAULE_BIN=... bash run_examples_diff.sh                      # 9/9 projects agree
```

Plus `cargo run --release -p saule-vm --example compare`, which asserts the
engines agree before timing.

`cargo` is not on PATH: use `C:\Users\lauri\.cargo\bin\cargo.exe`.
rustup's default toolchain is `-gnu`; msvc is active via a *directory
override* on the repo path, which a `git worktree` elsewhere does not
inherit. Fix: `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc cargo build`.

## What is left, in the order I would do it

### 1. Real-program coverage is the number that matters — it is stuck at 4 of 11

`tests/*.sau` is at 84 of 92 and the remaining fixtures are a scattered long
tail. `examples/` is the corpus that decides whether the VM engages on code
a user would write, and it has not moved. The seven that fall back, with the
**first** refusal in each:

| Project | Refusal |
|---|---|
| `json_usage`, `todo-app` | a `for … in` over an unproved source |
| `toying`, `UI Project` | an import of a dynamic native package |
| `fs-info-example` | a variant of an unknown enum |
| `ui-blocks` | a named argument to a callee the compiler cannot identify |
| `wrapper-types` | a class implementing `Assignable` |

First-refusal-wins, so closing one may only reveal the next — measure again
after each. The command is in `VM_TASKS.md`; note the `tr '\n' ' '` and that
a nested-backtick message defeats a naive `grep -o`, which cost me a wrong
reading this session.

**`for … in` over an unproved source is the largest and it has a trap.** The
obvious `ITERDRV` fix is written up in `VM_TASKS.md`: a table driver would
signal exhaustion with `nil`, but the table path does not use a nil
terminator, so a table holding a nil value would stop early under the driver
and iterate past it under the tree-walker. **Settle that before writing the
opcode.**

### 2. The remaining fixture long tail — 8, each independent

`an enum with methods` (needs §0.6's missing `NodeId` on enum methods, or a
different key), `a tuple pattern`, `a skipped parameter whose default must
run in the callee`, `a prelude name outside a call`, `a compound assignment
to a member`, `a class implementing Assignable`, `` `self` outside a
method``, `a declaration the compiler does not handle`. Cheap individually;
none unlocks another.

### 3. The open correctness items

- **A module-level forward call diverges.** `local r = later(5)` above
  `fn later` errors under the tree-walker and returns 105 under the VM. The
  compiler's proto pre-pass is correct *inside* function bodies and wrong
  for the module body's straight-line execution. Fix: track which top-level
  `fn`s the module body has passed, and only let `CALLK` resolve those.
  Written up in `VM_TASKS.md`. No fixture has this shape.
- **Two refusals multi-return left behind**, both deliberate and both
  documented: `return a, f()` (the returned range must be contiguous, and
  the allocator cannot put `f`'s window immediately after a live `a`), and
  `return x?.m()` (the nil arm cannot set `top`). Neither is wrong, but
  both cost a fallback.
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

**Do not start Phase 4 until real-program coverage is well above 4 of 11**,
or flipping the default just means most programs silently take the fallback.

### 6. Phase 5 — optimization, only with a profile in hand

Inline caches for `GETFX`/`CALLIF` (both now exist and are hash-probe
based), superinstructions from a measured histogram, precomputed hashes on
constant string keys, `get_unchecked` in the dispatch loop. `map` sits at
1.25x because it is hashing-bound inside `TableObject`, exactly as §20
predicts. A cheap one noticed this session: `local x = f()` emits the call
into a fresh window and then a `MOVE` to `x`, on every call form — a
peephole that lands the window on the destination would delete it.

## Working conventions that have paid off — please keep them

- **Differential testing is the discipline.** `crates/saule-vm/tests/
  differential.rs` runs every program under both engines and compares
  results *including error text*. Add cases there first.
- **Refuse rather than guess.** Anything codegen cannot handle returns
  `CompileError::Unsupported` naming the construct, and the CLI falls back.
  A wrong slot reads different data and nothing notices.
- **Reuse rather than reimplement.** `ARITHX` calls `ops::binary`, `CASTCHK`
  calls `cast`, `GETFX` calls `read_member`, `CALLMX` calls
  `dispatch_member_call_multi`, `CONCAT` calls `display_value`. Every time
  this rule was broken the engines diverged.
- **A missing type is never a wrong opcode** — it selects the dynamic form.
- **Opcodes are appended, never inserted.** The numbering is the chunk ABI;
  `encoding.rs`'s `opcode_numbering_is_stable` pins the last one and must be
  updated with each addition. Note that operand *encodings* are part of that
  ABI too: `CALLIF`'s `EXTRAARG` changed shape this session (it now packs
  the result count 8/16 with the interface index) without any opcode moving.
- Canaries to re-point when the named construct lands:
  `differential.rs`'s `unsupported_constructs_report_rather_than_miscompile`
  (currently `a tuple pattern`) and `handwritten.rs`'s unimplemented-opcode
  canary (currently `SUPER`).

## Four traps this codebase has already fallen into

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

## Uncommitted state

Everything through the VM work is committed at `4324c68`. Uncommitted on top
of it: the multi-return slice (`compile/expr.rs`, `compile/stmt.rs`,
`vm/mod.rs`, `tests/differential.rs`) plus this file and `VM_TASKS.md`. All
five verification commands pass as of this handoff.

Start with item 1 unless I say otherwise.
