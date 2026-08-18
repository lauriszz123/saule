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

Phases 0, 1, 2 **and 4** complete. Phase 3 is nearly complete. Phase 5
(optimization) is untouched.

**The VM is the default engine.** `saule run` uses it; `--interp` or
`SAULE_ENGINE=interp` selects the tree-walker, which stays in-tree as the
differential oracle. Everything still falls back cleanly, so the VM is safe on
any program — what is missing costs speed, not correctness. The fallback note
is now printed only when the VM was *asked* for (`--vm` / `SAULE_ENGINE=vm`),
which is what keeps the harnesses' fallback counts working.

| Measure | State |
|---|---|
| `tests/*.sau` compiling fully | **84 of 92** |
| `examples/**/*.sau` compiling fully | **10 of 61** |
| `examples/*` projects running fully on the VM | **5 of 9 compared** (2 skipped: interactive) |
| `benchmarks/sau` | 10 of 10 |
| Rust tests | **1405 passed, 0 failed, 5 ignored** |
| Differential tests | **191** |
| Release speed vs PUC Lua 5.4.8 | **1.0×–4.8×**, against the tree-walker's 5.5×–9.0× |
| Release speed vs the tree-walker | 2.0×–2.8× end-to-end; 2.63×–3.74× in-process |

The `examples/**/*.sau` row says 10, not the 17 the previous handoff claimed;
re-measured with the exit-status census at the flip. This is the third time
that number has been wrong in a handoff, in both directions.

**Numbers in a handoff go stale, and they drift in both directions.** One
handoff said "84 of 92" when `HEAD` was already better; the next said "85 of
92" when `HEAD` was 84 — `trailing_block_layout.sau` still falls back on the
skipped-default refusal and had been dropped from the list. Re-measure before
you plan around a number, and when you claim a change moved one, census
`HEAD` too: `git stash`, rebuild, count, `git stash pop`. Under two minutes,
and it is the difference between a measurement and a guess.

## Verify with all five — the last three are what catch VM bugs

```
RUST_MIN_STACK=16777216 cargo test --workspace          # fully green
SAULE_BIN=./target/debug/saule bash run_tests.sh        # 236/236, on the VM now
SAULE_ENGINE=interp SAULE_BIN=... bash run_tests.sh     # 236/236, the ORACLE
SAULE_ENGINE=vm SAULE_BIN=... bash run_tests.sh         # 236/236
SAULE_DIFF=1  SAULE_BIN=... bash run_tests.sh           # 236/236 + engines agree on OUTPUT
SAULE_BIN=... bash run_examples_diff.sh                 # 9/9 agree, 4 fall back
```

**The `interp` line is new and it is not optional.** A bare `run_tests.sh` used
to be the tree-walker's run and is now the VM's, so without it nothing covers
the oracle. CI has all five steps now; a local run should too.

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

### 2. The remaining fixture long tail — 8, each independent

`valued_enum` (an enum with methods — needs §0.6's missing `NodeId` on enum
methods, or a different key), `match_tuple` (a tuple pattern), `io_lib` (a
prelude name outside a call), `compound_assign` (a compound assignment to a
member), `assignable` (a class implementing `Assignable`), `closure_capture`
(`self` outside a method), `module_variable` (a declaration the compiler
does not handle), `trailing_block_layout` (a skipped parameter whose default
must run in the callee). Cheap individually; none unlocks another.

### 3. The open correctness items — all closed; what replaced them

Nothing on this list is a known divergence any more. What is left is
front-end work that the compiler is now waiting on.

- **`saule-typeck` models a value list as one value per expression.** The
  evaluator expands the last element of one (`eval_expr_list` →
  `eval_values`) and the typechecker does not, so `return 7, pair()` reports
  `` return value of type `(integer, integer)` is incompatible with declared
  return type `integer` `` and `local a, b, c = 7, pair()` reports `cannot
  assign nil to non-nullable type integer`. **Both run correctly under the
  tree-walker.** The bytecode compiler handles both shapes now; this is the
  only thing keeping them out of reach, and it is the same shape of gap the
  interface-return one was — the typechecker not modelling something the
  evaluator does.
- **A `?.` chain's own typing.** Not investigated, but worth a look while in
  the area: the same `Expr::Call` arm that could not see through an
  interface receiver dispatches on callee *shape*, and a safe method call's
  **arguments** are still never type-checked — `g?.twice("no")` passes
  typeck. Noted in §21.4 item 7 and still true.
- **The frame limit is aligned at the tree-walker's 10 000**, and raising it
  is Phase 4 work. §6.4's argument for a million frames is sound — a call
  here is a `Vec` push, not a native frame — but it has to move in both
  engines at once or `depth(50_000)` diverges again.

### 4. Testing gaps — do these before Phase 4

- **Verifier tests.** Seven exist (in `compile/verify.rs`); a bad opcode
  byte, a stray `EXTRAARG`, and out-of-range `Bx` per table are not covered.
- `www/` in the differential harness (`examples/` is done).
- Closure-semantics fixtures asserting values; memory fixtures with recorded
  peak-RSS bounds; a benchmark regression gate with the ~3% noise floor.

### 5. Phase 4 — **done**, with one box open and one warning it did not heed

Everything on the checklist landed except "one release ships with both
engines", which is a release and cannot be ticked from inside the tree — see
`VM_TASKS.md`'s Phase 4 for the per-item detail.

The warning this section used to carry — **do not start Phase 4 until
real-program coverage is well above 4 of 9** — was overridden rather than
satisfied. It was right about the risk and the risk is unchanged: 4 of the 9
comparable example projects still take the fallback, so for those the new
default is a no-op. It was never a *safety* argument (the fallback is
behaviour-preserving, and `SAULE_DIFF=1` plus `run_examples_diff.sh` both pass)
— it was a *value* argument, and closing item 1 above is what finally pays it.

Two things to know before touching this area:

- **A silent fallback is the point and also the hazard.** The note now only
  fires under `--vm`/`SAULE_ENGINE=vm`. If you are wondering whether the VM ran
  your program, ask it explicitly; a quiet run proves nothing either way.
- **`saule-vm` now has a `native-packages` feature**, a passthrough to the
  interpreter's, and `saule-wasm` depends on the VM with
  `default-features = false`. Turn that off and wasm32 stops building, because
  `libloading` comes back in through the VM. The check is
  `cargo check -p saule-wasm --target wasm32-unknown-unknown` — a host
  `cargo build` passes regardless.

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
  `TAILCALLS`) and must be updated with each addition. Operand *encodings*
  are part of that ABI too.
- Canaries to re-point when the named construct lands:
  `differential.rs`'s `unsupported_constructs_report_rather_than_miscompile`
  (currently `a tuple pattern`) and `handwritten.rs`'s unimplemented-opcode
  canary (currently `SUPER`).

## Six traps this codebase has already fallen into

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
6. **A "not yet — it would diverge" note expires when the other engine
   moves.** `VM_TASKS.md` argued for years-of-reading that `TAILCALL` must
   wait, because a tail-recursive loop would run unbounded under `--vm` and
   overflow without it. True — until the tree-walker got a trampoline, at
   which point the same note was defending the divergence it was written to
   prevent. **Every deferral justified by "the engines would disagree" is
   relative to what the other engine does today.** When you change the
   tree-walker, grep `VM_TASKS.md` for the deferrals that reasoning
   supported.

## Uncommitted state

Everything is committed at `3a9b6f7`. Uncommitted on top of it, in two
slices:

**Tail calls.** `op.rs` (`TAILCALLK`/`TAILCALLS` appended, `TAILCALL`
documented), `vm/mod.rs` (`enter_tail_frame` plus three arms),
`compile/expr.rs` (`Want::Tail`), `compile/stmt.rs` (the two vetoes in
`ret`, `try_depth`), `compile/ctx.rs`, `compile/verify.rs`,
`tests/encoding.rs`. Plus two bugs it uncovered:
`crates/saule-interpreter/src/eval/stmt/try_.rs` (a forced tail call escaped
its own handler) and `tests/ui/stack_overflow_recursion.sau` (tail recursion
turned the fixture into a hang, stalling `run_tests.sh`, which has no
per-fixture timeout).

**The open correctness items.** `compile/expr.rs` and `compile/stmt.rs`
(`SETFX`, the dynamic safe read/call, `return x?.m()`, `return a, f()`,
`reaches_undeclared`), `compile/mod.rs` (the reference-graph pre-pass),
`vm/mod.rs` (`SETFX`), `saule-ast/src/{lib,visit}.rs` (`visit_stmts`),
`saule-interpreter/src/lib.rs` + `eval/stmt/{assign,mod}.rs`
(`write_member_dynamic`), `saule-semantic/src/{lib,registry}.rs`
(`InterfaceMethodRegistry`), `saule-interpreter/src/module/seed.rs`,
`saule-typeck/src/expr/infer.rs`.

**Phase 4, the default flip.** `saule-cli/src/{cli,main,run}.rs` (`--interp`,
`Engine`, `select_engine`, the note gate), `saule-wasm/{Cargo.toml,src/lib.rs}`
(the VM path, `Phase::Compile`), `saule-vm/Cargo.toml` (the `native-packages`
passthrough), `run_examples_diff.sh` (portable `SAULE_BIN` default plus the
missing-binary guard), `.github/workflows/ci.yml` (the four engine-mode steps
the flip made necessary), `README.md` ("Execution Engines"), `PRODUCTION.md`,
`VM_TASKS.md`, this file.

All the verification commands pass as of this handoff, and `cargo test
--workspace` is 1405/1405.

**Two CI gates are red at `HEAD` and were red before any of this.** Neither is
from the work above; check before blaming a change.

- `cargo fmt --all --check` — ~180 hunks, most of them in `saule-vm`, but also
  in files untouched since `HEAD` (`saule-ast/src/ids.rs`,
  `saule-cli/src/check.rs`, `saule-typeck/src/table.rs`).
- `cargo clippy --workspace --all-targets -- -D warnings` — one
  `single_match` in `saule-typeck/src/coverage.rs:105` (unmodified since
  `HEAD`) and 8 in `saule-vm`, mostly `match_single_binding` in the dispatch
  loop.

`saule-cli` and `saule-wasm` are clean under both. Clearing the rest is worth
its own commit — a 180-file `cargo fmt` mixed into feature work buries it.

Start with item 1 unless I say otherwise. It is now the *only* thing standing
between the default flip and the flip mattering.
