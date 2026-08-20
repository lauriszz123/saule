# Saule VM — Task List

> The execution plan for `VM_DESIGN.md`. That document is the *specification*;
> this one is the *checklist*. Section references (§) point back into it.
>
> **Ground rule, absolute:** `./run_tests.sh` passes at every commit, and the
> tree-walker stays in-tree and green. It was the default engine until Phase 4
> flipped that; it is still the differential oracle, which is the harder
> requirement of the two.
>
> That means **all four modes**, because each catches what the others
> cannot:
>
> ```
> ./run_tests.sh                       # the default engine — the VM, since Phase 4
> SAULE_ENGINE=interp ./run_tests.sh   # the tree-walker still works
> SAULE_ENGINE=vm ./run_tests.sh       # the VM runs or cleanly falls back
> SAULE_DIFF=1 ./run_tests.sh          # the two agree on *output*, not just exit status
> ```
>
> The last was added late and immediately found bugs the others had been
> passing over for months — exit status alone cannot see a wrong value.
>
> The first two swapped meaning at the flip: a bare `run_tests.sh` used to be
> the tree-walker's run and is now the VM's, so `SAULE_ENGINE=interp` is what
> keeps the oracle covered. Losing that would not fail anything — it would
> just quietly stop testing half of what this file is about.

## Verifying a change

Five commands. The last three are the ones that catch VM bugs; the first two
catch everything else.

```
cargo test --workspace                                  # fully green, nothing excluded
SAULE_BIN=./target/debug/saule.exe bash run_tests.sh    # 236/236, on the VM by default
SAULE_ENGINE=interp SAULE_BIN=... bash run_tests.sh     # 236/236, the oracle
SAULE_ENGINE=vm SAULE_BIN=... bash run_tests.sh         # 236/236
SAULE_DIFF=1  SAULE_BIN=... bash run_tests.sh           # 236/236 + engines agree on output
```

Plus two more:

```
SAULE_BIN=./target/debug/saule.exe bash run_examples_diff.sh   # 9/9 agree, 4 fall back
cargo run --release -p saule-vm --example compare              # agrees, then times
```

`run_examples_diff.sh` runs the *example projects* under both engines —
multi-module, with imports and file IO — which is a different question from
`run_tests.sh`'s single-file fixtures, and the one that has actually caught
things.

### Per-platform notes

On the Windows box `cargo` is not on `PATH` — use
`C:\Users\lauri\.cargo\bin\cargo.exe`, and the binary is `saule.exe`.

On macOS the binary is `./target/debug/saule` (no `.exe`), and two things
bite that do not on Windows:

- **`cargo test --workspace` needs `RUST_MIN_STACK=16777216`.** Without it
  `the_recursion_guard_still_unwinds_after_re_entrant_calls` overflows
  libtest's 2 MiB test thread and aborts the whole binary — including at
  `HEAD`, so it is a platform floor, not a regression. Its own comment
  already says the guard needs more real stack than libtest gives; macOS is
  simply where that bill comes due. Check `HEAD` before blaming a change.
- **`run_examples_diff.sh` needs no GNU `timeout`.** Stock macOS has
  neither `timeout` nor `gtimeout`, and without one every project failed
  identically — which the harness faithfully reported as *"9 of 9 projects
  disagreed"*. A divergence that large and that sudden is far more likely
  to be the harness than the engine; the script now prefers coreutils and
  falls back to a shell watchdog, so it runs the same everywhere.

## Working conventions that have paid off — please keep them

- **Differential testing is the discipline.**
  `crates/saule-vm/tests/differential.rs` runs every program under both
  engines and compares results *including error text*. Add cases there first.
- **Refuse rather than guess.** Anything codegen cannot handle returns
  `CompileError::Unsupported` naming the construct, and the CLI falls back to
  the tree-walker. A wrong slot reads different data and nothing notices;
  a refusal costs speed, never correctness.
- **Reuse rather than reimplement.** `ARITHX` calls `ops::binary`, `CASTCHK`
  calls `cast`, `GETFX` calls `read_member`, `CALLMX` calls
  `dispatch_member_call_multi`, `CONCAT` calls `display_value`, `LEN` defers
  to `ops::unary`, and `ITERPREPX` calls `call_member_dynamic` for an
  instance's `iter()`. Every time this rule was broken the engines diverged.
  (But read trap 5 — reuse the *logic*, not the *type tests*.)
- **A missing type is never a wrong opcode** — it selects the dynamic form.
  The `X` suffix is that convention: `ARITHX`, `UNARYX`, `GETFX`, `CALLMX`,
  `ITERPREPX`.
- **Opcodes are appended, never inserted.** The numbering is the chunk ABI,
  and it will matter the day §14's bytecode cache lands. `encoding.rs`'s
  `opcode_numbering_is_stable` pins the first four and the last one (now
  `TAILCALLS`) and must be extended — not edited in the middle — with each
  addition. Operand *encodings* are part of that ABI too.
- **Two canaries to re-point when the construct they name finally lands:**
  `differential.rs`'s `unsupported_constructs_report_rather_than_miscompile`
  (currently a tuple pattern) and `handwritten.rs`'s unimplemented-opcode
  canary (currently `SUPER`). Both assert that the *designed* failure still
  happens; when the construct lands, they need a new stand-in rather than
  deletion.

## Nine traps this codebase has already fallen into

Each one cost real debugging time and each is written up in full further
down; this is the index, so you meet them before you repeat them.

1. **A module-level `local` is a module *slot*, not a frame local**, so
   `FuncCtx::lookup` structurally cannot see it. Three shadowing bugs came
   from checks built on that lookup alone — `local Math = {pi: 3.0}` reading
   π, `local String = {…}` calling the stdlib's `String.len`, a `local`
   shadowing a class reading the class's static. Use
   `Compiler::not_shadowed`, which checks both places a `local` can land, or
   the resolver's `Binding::Prelude` where that is the precise question.
   (Phase 3 item 8, "Stdlib value members and table dot access"; the same
   ordering trap is called out again in item 9, "Pipes".)
2. **Widening what compiles can turn an inert documented gap into a live
   divergence.** `EnumObject.methods` was empty and *documented as safe*
   because enum-method calls refused — then `CALLMX` dispatched dynamically
   and reached the empty map. A dynamic fallback is only as safe as the
   runtime data it falls back *onto*. Re-read the "known safe because X
   refuses" notes whenever you delete a refusal. (§ "§8.5 dynamic member
   dispatch — done, and what it exposed".)
3. **A reported refusal can be hiding an unreported miscompile.** Chasing
   `case x when x < 0` (a clean refusal) found a second bug beside it: a
   failed pattern jumped into the arm body whenever the arm had a guard —
   wrong value, exit status 0, invisible to `SAULE_DIFF=1` because no
   fixture paired a *literal* pattern with a guard. A refusal says the
   compiler mishandles a construct; it does not say the mishandling is
   confined to the part that refused. (§ "Two bugs in `match` guards — one
   silent".)
4. **Correct-by-accident survives exactly as long as nothing can observe
   it.** `return f()` truncated a callee's results to one, which was
   invisible while nothing compiled could consume two — and became a wrong
   value the moment a `for … in` driver could. Before widening coverage, ask
   what the *narrowness* was quietly making safe. (§ "Multi-return and
   parallel binding — done, and the divergence it exposed".)
5. **Copying the oracle verbatim is wrong where the two engines represent a
   value differently.** `ITERPREPX`'s callable test was lifted from
   `exec_for_in`, which lists `Function`/`Native`/`NativeClosure` and never
   `VmFunction` — because the tree-walker cannot construct one. Under the VM
   every compiled closure *is* a `VmFunction`, so the first cut refused every
   driver on the new path. Reuse the oracle's *logic*; check its *type tests*
   against this engine's value representation. (Phase 3 item 5, "`for … in`".)
6. **A "not yet — it would diverge" note expires when the other engine
   moves.** This file argued at length that `TAILCALL` must wait, because a
   tail-recursive loop would run unbounded under `--vm` and overflow without
   it. True — until the tree-walker got a trampoline, at which point the same
   note was defending the divergence it was written to prevent. **Every
   deferral justified by "the engines would disagree" is relative to what the
   other engine does today.** When you change the tree-walker, grep this file
   for the deferrals that reasoning supported. (§ "Tail calls — done, and the
   two bugs it uncovered".)

7. **A comment that asserts one property of several cases is a claim about
   each of them, and it will be read as documentation.** `Compiler::type_desc`
   collapsed both `Type::Tuple` and `Type::Nullable` to `TypeDesc::Any` under
   one sentence — "not a runtime test the tree-walker performs either".
   `Tuple` really is `true` in `runtime_matches_type`; `Nullable` is
   `nil || inner`, three lines further down the same `match`. So
   `catch e: string?` caught a thrown integer under the VM and let it escape
   under the tree-walker: silent, exit status 0, and no fixture exercised it.
   Check a multi-case claim case by case, against the oracle's own `match`.
   (§ "A nullable `catch` type caught everything".)
8. **The refusal message names the construct, not the cause.** Three of
   Phase 3's last four gaps were misread from their own wording.
   `a declaration the compiler does not handle` was a missing `match` arm for
   a node the resolver had already given a slot; ``self` outside a method`
   was `in_method` asked of a *lambda's* frame; `an enum with methods` was a
   `HashMap<String, Rc<FunctionObject>>` that a bytecode method could not
   inhabit — and this file's own note against it blamed §0.6's missing
   `NodeId`, which would have meant changing the AST for no reason. Read the
   refusal as "here is where compilation stopped", never as "here is what is
   missing". (§ "The last four gaps — closed".)

9. **An opcode existing is not an opcode being emitted, and a comment
   saying "the compiler uses X here" is not evidence that it does.**
   `JLTI`–`JGEF` were implemented in the dispatch loop in Phase 1, covered by
   the opcode tests, and **never emitted once**. What kept that invisible for
   four phases was a comment in `binary_opcode` asserting the opposite —
   "the fused branch forms are used where the value feeds an `if`, which
   `stmt::cond_jump` handles" — next to the code it was describing. Nothing
   fails when an optimization silently does not happen: the program runs, the
   differential tests agree, the disassembly is the only witness. Assert on
   the **emitted code** for anything whose whole purpose is to not be there.

   **Three instances in one week**, all found by reading a profile rather
   than by any test failing: `JLTI` and its eleven siblings, the
   `ADDII`/`SUBII`/`MULII` immediate family, and `if_chain`'s "only worth a
   jump to the end when something follows" — which emitted one every time.
   In each case a comment stated the optimization as fact. Treat a comment
   that describes what the compiler emits as a **hypothesis**, and check it
   against `disasm`. (§ Phase 5, "§17 emission peepholes".)

## Legend

| Mark | Meaning |
|---|---|
| `[x]` | done and tested |
| `[~]` | partially done — see the note |
| `[ ]` | not started |

## Where things stand

**Phases 0–4 are complete.** The compiler turns Saule source into bytecode and
the VM runs it **1.2x–3.3x** faster than the tree-walker end to end (geometric
mean **2.2x**), with **216** differential tests asserting the two engines
agree — plus 236 fixtures, 11 example projects and 20 `www/` samples run under
both engines and compared by output.

**Phase 5 has started. §17's emission peepholes are done, and the one
superinstruction a profile ever supported has shipped.** Not the inline
caches this phase was written around — `--profile-bytecode` chose these
instead, and the numbers are under the items.

Instructions retired, which is the figure that is not a stopwatch:
`loop_arith` **−50%**, `sort` **−47%**, `mandel` **−45%**, `fib` **−35%**,
`closure` −20%, `array` −18%, `oop` −11%. On the clock, net of process
start-up: `loop_arith` −31%, `mandel` −21%, `fib` −21%, `sort` −10%.

**The two halves of that read differently, and the difference is the useful
part.** The peepholes were six changes with **no new opcodes** — every one
the compiler emitting instructions the VM already had, three of which had
never been emitted once since Phase 1 — and they moved both columns
together. `CASTUNWRAP` cut `sort`'s instruction count by a further 30% and
moved its clock by 2.3%, which is §20's prediction arriving with numbers
attached: what is left in `map` and `sort` is `TableObject` and the
engine-boundary crossing, not dispatch. **Stop optimising dispatch for those
two.**

The one Phase 4 box that cannot be ticked from inside the tree is still open:
a release has to actually ship.

**Two trap entries came out of this week and are worth reading before
optimizing anything else:** a claim in a comment that covers several cases is
a claim about each of them (§ trap 7), and a refusal message names where
compilation stopped, not what is missing (§ trap 8).

That range is wider at both ends than the "2.6x–3.7x" this line used to
carry, and the difference is *platform*, not regression: the old figure came
from the in-process `compare` example on Windows x86_64, this one from
`bench.py`'s ten programs on macOS arm64. Both are recorded, per machine and
per Lua version, in `PRODUCTION.md` Appendix A — do not average them.

**The VM is the default engine** as of Phase 4. `saule run` uses it; `--interp`
or `SAULE_ENGINE=interp` selects the tree-walker, which stays in-tree as the
differential oracle. Nothing about the fallback changed — a module the
compiler cannot reach still runs on the tree-walker, silently now rather than
with a note, so the flip is safe on any program. `--vm` restores the note.

**Phase 4 did not change coverage — but the end of Phase 3 did.** That
paragraph used to read "4 of the 9 comparable example projects still take the
fallback, so for those the new default is a no-op", and it is now **0 of 9**:
every comparable project runs entirely on the VM. Phase 4's "What flipping
the default did *not* fix" is worth reading for the reasoning, not for its
numbers.

**Phase 3 is complete.** Classes, interfaces, enums + `match`, `try`/`catch`,
`for … in` (table path), operator overloading (left operand, including unary
and index), **nullability** (`?.`, `??`, `!`, `as`), stdlib value members,
table dot access, pipes, imports/modules, §19 argument binding, the
`ARITHX`/`UNARYX` dynamic fallback, and **VM re-entrancy** are all done.

The paragraph above used to end "Remaining in §21.4 order: pipes,
imports/modules, §19 argument binding" — which was stale by three items when
it was read. Those three had been checked off below without this summary
being touched, which is the second time this file has carried a status line
that its own item list contradicted. **Re-read the items before trusting the
summary.**

The last four real gaps closed together, and they were four unrelated one-line
premises rather than one missing feature:

| Gap | What it actually was |
|---|---|
| `a declaration the compiler does not handle` | `Decl::Variable` (`export name: T = value`) had no branch. The resolver already gave it a module slot. |
| ``self` outside a method` | The test was `in_method` on the *lambda's* frame, which is never a method. `self` is a capturable local named `self`. |
| `a prelude name outside a call` | `Io.stdout` is an object, not one of the scalars `prelude_member` folds, so the bare `Io` had to become a value. |
| `an enum with methods` | `EnumObject::methods` could only hold a tree-walker `FunctionObject`. |

Each is written up below, along with a **live silent divergence** the last of
them turned up next door: `catch e: string?` caught a thrown integer under
the VM and let it escape under the tree-walker.

**Coverage, measured rather than inferred.** "236/236 under
`SAULE_ENGINE=vm`" counts a fallback as a pass, so it is not the number to
steer by. The real one:

| | Compiles fully | Falls back |
|---|---|---|
| `benchmarks/sau` | **10 of 10** | — |
| `tests/*.sau` | **91 of 92** | 1 |

(Re-measured at the close of Phase 3. It read 87 before, and at the Phase 4
flip 85, where the line above it said 84 — three successive handoffs slipped
this number in both directions. Re-count before planning around it; the
census commands are below.)

**The one that falls back is not a gap.** `tests/compound_assign.sau` refuses
on `a compound assignment whose target cannot be evaluated only once`, and
that refusal is what *fixes* a miscompile — see "Compound assignment" below.

**And the same measurement on real code, which says something different.**
`tests/*.sau` are single files; every real Saule program is a project with
imports, so the fixture ranking below is not the ranking that decides
whether the VM engages on anything a user would write.

| Corpus | Compiles fully |
|---|---|
| `examples/**/*.sau` | **12 of 61** — but see the box above; this number does not mean what it looks like |
| `examples/*` projects, end to end | **9 of 11** run fully on the VM. `run_examples_diff.sh` reports **0 fallbacks**; the 2 that remain are `toying` and `UI Project`, refused by *design* on `an import of a dynamic native package` |

The project row is the one Phase 4 turned into a headline: it is the fraction
of real programs for which the new default engine is the engine that actually
runs, and `run_examples_diff.sh` prints it on every run.

**Count with `disasm`'s exit status, not by parsing its message.** Every
message-parsing form tried here has under-counted refusals, which is the
direction that makes the work look done:

```
n=0; t=0
while IFS= read -r f; do
  t=$((t+1))
  ./target/debug/saule disasm "$f" >/dev/null 2>&1 && n=$((n+1))
done < <(find examples -name '*.sau')
echo "$n of $t"
```

Three successive attempts got this wrong, each more subtly than the last:

1. A **line-oriented `grep`** scored a refused file as compiling, because
   `miette` wraps a long message across lines. Result: "50 of 61", a fivefold
   overstatement. Fixed with `tr '
' ' '`.
2. **`grep -o '`[^`]*`'`** truncated the messages that *contain* backticks —
   `` `a class implementing `Assignable`` ``, `` ``self` outside a method`` —
   into fragments that then sorted as their own bogus causes.
3. **A `sed` anchored on a trailing phrase** — `'s/.*× \(.*is not supported\).*/\1/p'`
   — missed any message whose wrap falls *between* the closing backtick and
   `is not supported`: after `tr`, the line reads
   ``...callee` is not | supported by...``, and the pattern does not match.
   Result: "17 of 61" against a true 10, with two whole causes invisible —
   `a named argument to a callee the compiler cannot identify` (5 files) and
   `a class implementing an interface this compiler cannot see` (2).

For a **cause histogram** you still have to read the message, so squeeze the
wrap out first and anchor on `×` alone rather than on any trailing phrase:

```
while IFS= read -r f; do
  ./target/debug/saule disasm "$f" 2>&1 >/dev/null \
    | tr '\n' ' ' | tr -s ' ' \
    | sed -n 's/.*× `\([^`]*\)`.*/\1/p' | head -1
done < <(find examples -name '*.sau') | sort | uniq -c | sort -rn
```

*(The `head -1` has to be inside a command substitution, not on the end
of the pipeline: `sed -n ...p` emits no trailing newline for a one-line
match, so writing it inline concatenated all 48 causes onto a single
line that `uniq -c` then counted as one. A fourth way to get this
census wrong — wrap the pipeline in `c=$(...)` and `echo "$c"`.)*

Cross-check its total against the exit-status count. If they disagree, trust
the exit status — a cause you cannot parse is still a refusal.

| Cause (first refusal per file) | Files |
|---|---|
| a class extending one the compiler cannot see | **24** |
| an import declaration | **10** |
| a name the resolver could not classify | 9 |
| a named argument to a callee the compiler cannot identify | 4 |
| a class implementing an interface this compiler cannot see | 1 |
| *(not a refusal — see below)* | 1 |

Re-censused at the close of Phase 3. `a variant of an unknown enum` and
`a class implementing `Assignable`` are both gone; the standalone count rose
from 10 to **12 of 61**. An earlier version of this table read 8 / 5 / 2 and
carried a `a skipped parameter whose default must run in the callee` row that
belongs to `tests/*.sau`, not here — it is not a cause in `examples/` at all.

**One of the 49 is not a refusal.** `examples/todo-app/src/storage.sau` fails
`disasm` with `cannot determine the type of this expression` on
`Json.encode(data)` — a *typecheck* error, because `disasm` compiles one file
without its import graph and cannot see `Json`. The census counts it as a
failure because it counts exit status, which is the right rule; just do not
read it as a compiler gap. True refusals are **48**.

First-refusal-wins, so a cause that only appears late in a file is
under-counted.

> **This per-file census does not measure what it was being read as
> measuring, and an earlier version of this section drew the wrong
> conclusion from it.** `disasm` compiles **one file**, through the
> single-module path; a real program compiles through `program::compile`,
> which walks the import graph. So a file that refuses standalone may be
> perfectly compilable as part of its project, and most of them are:
>
> * All **24** `a class extending one the compiler cannot see` files are in
>   one project, `UI Project` — which falls back earlier anyway, on the
>   *deliberate* `an import of a dynamic native package` refusal, and never
>   reaches class layout.
> * All **10** `an import declaration` files are refusing by design: a lone
>   `import` on the single-module path is a documented correctness rule,
>   pinned by `an_import_without_a_program_driver_still_refuses`.
> * Five files across `vector-math`, `json_usage`, `bitwise-flags`, `bf` and
>   `json` refuse standalone while **their projects run fully on the VM with
>   no fallback at all**.
>
> So "34 of 50 refusals are the cross-module slice, the only thing of its
> size left" was wrong: the cross-module slice had already been closed by the
> imports work, and this census could not see it. **Steer by the per-project
> table below, not by this one.** This one is still worth keeping — it is a
> cheap smoke test of the single-module path — but it is not a coverage
> measurement for real code.

Every remaining cause, by the fixtures it blocks. Regenerate this with:

```
for f in tests/*.sau; do SAULE_ENGINE=vm ./target/debug/saule.exe run "$f" 2>&1 \
  | grep -o "does not handle .*yet"; done | sort | uniq -c | sort -rn
```

| Cause | Fixtures | Note |
|---|---|---|
| a compound assignment whose target cannot be evaluated only once | 1 | **not a gap** — the refusal is what fixes a miscompile; see "Compound assignment" below |

**One, down from eight, and it is the one that should stay.** Closed at the
end of Phase 3: `an enum with methods`, `a prelude name outside a call`,
`a declaration the compiler does not handle` and ``self` outside a method`,
each written up below. Closed before it: `a tuple pattern`, `a skipped
parameter whose default must run in the callee`, `a class implementing
`Assignable`` and `a compound assignment to a member`.

`§0.6's missing NodeId, or a different key` — the note this table carried
against `an enum with methods` — was the wrong diagnosis, and following it
would have meant changing the AST. The `NodeId` was never what blocked it;
see the write-up.

*Fixed since:* `case x when …` was right — it was a bug, not a gap, and it
was hiding a second, silent one. See "Two bugs in `match` guards" below.
**Multi-return / parallel `local a, b = f()`** closed four fixtures at once
(`fibonacci`, `string_lib`, `call_return_inference`,
`multiple_return_values`) and, along the way, a live silent divergence —
see "Multi-return and parallel binding" below.

Gone from this table since re-entrancy landed: `classes with a toString
overload` (2), `function value passed to a built-in` (1), and `methods on a
stdlib class instance` (2).

---

# Phase 0 — Foundations

*Estimate: 2–4 weeks. Every item is a standalone improvement to the existing
interpreter. If the VM were cancelled today, all of this still pays for
itself.*

### 0.1 `NodeId` on `Spanned` — §21.1

- [x] Add `NodeId(u32)` with `NodeId::NONE`, and an `id` field on `Spanned<T>`
      — `crates/saule-ast/src/lib.rs`
- [x] Hand-write `PartialEq` for `Spanned` so it **ignores `id`** — it is
      derived today and parser tests compare parsed trees against hand-built
      ones; deriving over the new field breaks every one of them
- [x] `saule_ast::assign_ids(&mut Module)` — deterministic pre-order
      numbering, called once after parsing
- [x] Add `id: NodeId::NONE` to the 13 `Spanned` struct-literal sites
      (`saule-parser`, `saule-lexer`, `saule-fmt`, `saule-cli`). The 73
      `Spanned::new` call sites stay untouched.

*Independent value:* the LSP can key caches on node identity, not spans.
*Risk:* low, mechanical.

### 0.2 Slot-based instance fields — §4.4, §21.1

- [x] `FieldLayout { index: HashMap<Rc<str>, u16>, names: Vec<Rc<str>> }`,
      shared per class via `Rc` on `ClassObject`
- [x] `InstanceObject.fields: HashMap<String, Value>` → `Vec<Value>`
- [x] Build the layout at class construction, **parent fields in the low
      slots**, with a redeclared field keeping the parent's slot so the
      prefix rule survives shadowing
- [x] `read_member` → `layout.slot(name)` + indexed load
- [x] `init_fields` fills slots, field assignment goes through the layout,
      `Os.fsInfo`'s synthetic `FsInfo` class got a real layout
- [x] Writing an undeclared field is now a clean runtime error instead of
      silently creating an unreachable one (the typechecker already
      rejected it; this is the unchecked `run()` path)

*Note:* `init_fields` picks a method's closure to evaluate field defaults in.
Flattening (0.3) put the parent's methods in that pool, whose closures capture
a different module scope — so the pick is now filtered on `resolved_owner`.
Easy to miss; covered by a test.

### 0.3 Flattened method tables — §8.3, §21.1

- [x] `methods` and `static_methods` built as a copy of the parent's with
      overrides written in; `lookup_method` is one probe at any depth
- [x] Interface validation runs *before* flattening, so which classes satisfy
      an interface is unchanged
- [x] Owner back-links set for **own methods only** — an inherited entry is
      the same `Rc<FunctionObject>` the parent holds, so re-pointing it would
      rewrite the parent's method to belong to the subclass
- [~] `vtable` / `vindex` on `ClassObject` — **deliberately skipped.** §24.2
      resolves the divergence risk by making the runtime class be built *from*
      the compiler's `ClassProto`, not computed alongside it. Adding a second,
      independently-computed vtable now would be exactly the divergence that
      section warns about, and the tree-walker would never read it. The
      `FieldLayout` is the part that genuinely needs to exist early, and it
      does.

### 0.4 Dense enum tags — §9.1, §21.1

- [x] `EnumVariantObject.tag: u32`; `EnumObject.by_tag`, `tags`,
      `tag_of`, `variant_by_tag`, `variant_count`
- [x] Tags assigned in declaration order, including tuple variants (which
      have a tag but no singleton, so `by_tag` records `None`)
- [x] A constructed `Event.Click(x, y)` carries its declaration's tag,
      resolved once when the constructor is built rather than per call
- [x] `Os`/`Io` stdlib enums: variant lists hoisted to constants so the
      installer and the fresh-construction sites cannot drift on tag order
- [~] `match_.rs` comparing tags instead of strings — **not done, on
      purpose.** A pattern carries *names*, so the tree-walker would have to
      hash the variant name to get its tag, which is slower than the short
      string compare it does today. Tags pay off when the *compiler* resolves
      the pattern's tag ahead of time and emits `SWITCH` (§9.2). The data is
      in place for that; the tree-walker's test is left alone.

### 0.5 Typeck publishes a type table — §21.1

- [x] `pub type TypeTable = HashMap<NodeId, Type>`
- [x] `check_with_types(&Module) -> (Vec<TypeCheckError>, TypeTable)`, with
      `check` reimplemented on the same walk — **no caller changes**
- [x] Recording hangs off the single `infer` entry point via a thread-local
      sink, consistent with how the crate already carries `CURRENT_CLASS` /
      `RETURN_TY`. The sink is `None` during a plain `check`, so the language
      server's per-keystroke path costs one thread-local read and allocates
      nothing.
- [x] `saule check --dump-type-coverage`, reporting per-file and total
- [x] **Baseline measured and recorded** (below)

*Independent value:* inlay hints and precise hover types in the LSP.

#### Coverage baseline — recorded when 0.5 landed

| Corpus | Expression nodes typed | Arithmetic operands typed | …of which concretely `integer`/`float` |
|---|---|---|---|
| `benchmarks/sau` | 199/290 (68.6%) | 72/72 (100%) | **72 (100%)** |
| `examples` | 19592/27872 (70.3%) | 1828/1852 (98.7%) | **1792 (96.8%)** |
| `tests` | 3136/5350 (58.6%) | 285/292 (97.6%) | **271 (92.8%)** |

The last column is the one §24.1 sets a **~90% bar** on, because it is what
decides `ADDI` versus the dynamic `ARITHX`. Every corpus clears it, and the
benchmark set — the programs Phase 2's exit criterion is measured on — is at
100%. **§24.1 is retired as a project risk.**

Overall expression coverage being much lower is expected and is not the same
question: the untyped nodes are overwhelmingly calls and member reads, which
`infer` is documented as declining to see through. A missing entry costs a
dynamic opcode, never a wrong one.

A regression guard on the arithmetic figure lives in
`crates/saule-interpreter/tests/type_table.rs`, alongside a test that
`check_with_types` produces byte-identical diagnostics to `check`.

### 0.6 Semantic publishes a binding table - 21.1

**Part A - publish the table. Done.**

- [x] `Binding` enum (`Local`/`Upvalue`/`Module`/`ClassStatic`/`Prelude`/
      `SelfRef`/`WildcardImport`), `ResolveTable`, `FunctionTable`,
      `UpvalRef`, `FunctionInfo`, bundled as `Bindings`
- [x] `analyze_with_bindings(&Module, ModuleSeed) -> (Vec<SemanticError>, Bindings)`,
      with `analyze_with_seed` reimplemented on top - **no caller changes**
- [x] Scope frames rebuilt as two nested stacks: **function** scopes, and
      **block** scopes within each. Every local gets a frame slot; leaving a
      block returns its slots, so sibling blocks share registers (18)
- [x] Module-level names collected in **declaration order** - the index is
      the module slot, and a `HashSet`'s hash-seed-dependent order would move
      slot numbers between runs and break a bytecode cache
- [x] **Real free-variable analysis**: the Lua capture algorithm, adding a
      link to every function between the reference and the frame that owns
      the variable. A name captured twice gets one upvalue; a name reached
      across two boundaries is threaded through the middle closure rather
      than grabbed past it.
- [x] Collection is opt-in, so `analyze_with_seed` - which the language
      server runs per keystroke - builds nothing

*Deviation:* `Binding::Local` carries a slot but no `frame_depth`. Block
depth is a compile-time notion; by the time a name resolves, the slot is
already absolute within the frame, and anything outside the current function
is an `Upvalue` rather than a deeper local.

*Known gap:* an enum method is a bare `Method` in the AST with no `Spanned`
wrapper, so there is no `NodeId` to key its `FunctionInfo` on. Resolution and
diagnostics are unaffected; only the compiler's frame lookup is, and it can
compute that itself until the AST grows a span there.

*Nearly-introduced regression, now covered by a test:* a `local` inside a
block at top level is **not** a module slot. Treating it as one made
`if x then local y = 1 end` leak `y` into module scope.

**Part B - adopt it in the tree-walker. Done.**

- [x] `saule_interpreter::analyze_and_prepare` runs `analyze_with_bindings`
      and publishes the capture sets; the four execution paths
      (`check_and_run_in`, the module loader, `saule-cli run`, `saule-wasm`)
      go through it. The LSP, which never executes, is untouched.
- [x] `capture.rs`'s over-approximating walker **deleted**. What remains of
      the file is a registry: the pipeline hands over the answer, lambda
      evaluation looks it up.
- [x] The bail-out is gone. A nested `Stmt::Decl` no longer forces
      whole-scope capture.
- [x] `self` handled: it is not an identifier so it never appears in an
      upvalue list, and the resolver tracks it separately - marked on **every**
      enclosing function, since a lambda two levels inside a method reaches
      `self` through the one between them.
- [x] Fallback preserved: a module that was never analysed still captures its
      whole scope, so the raw `run` / `run_in` entry points stay correct.
- [x] 17 value-asserting closure tests
      (`crates/saule-interpreter/tests/closure_semantics.rs`), including a
      direct assertion that the closure's environment no longer holds a
      binding its body never referenced - the leak itself, checked rather
      than inferred.

*Design note:* the registry is keyed on the lambda body's `Arc` address, not
its `NodeId`. Node ids are per module and every module numbers from zero, so
a `NodeId` alone would collide across modules - and any program with imports
evaluates lambdas from several. The address is globally unique while the body
is alive, and the entry holds the body to keep it so. It is also the key the
old memo used, and for the same second reason: a lambda written inside a loop
shares one `Arc` body across iterations, so one entry serves every closure
built from it.

*Statics were the near-miss:* narrowing the capture set would have broken
bare-name static reads from a lambda inside a method, except that statics do
not travel through the capture set at all - they hang off the scope's
`statics_owner`, which `capture_flat` preserves. Pinned by a test now.

Part A was pure additionPart A is pure addition: every existing signature still exists and every
diagnostic is byte-identical (asserted). Part B changes how closures capture
at runtime, which is a different risk class, so it is sequenced separately.

### Phase 0 exit criteria

- [x] `./run_tests.sh` green - all 235 fixtures
- [x] Benchmark comparison against the pre-Phase-0 commit, measured
- [x] `Value` is still 16 bytes (guarded by a test)
- [x] Type coverage recorded as a baseline (see 0.5)

#### Toolchain note

The baseline build failed for a while with `Error calling dlltool
'dlltool.exe': program not found`. Cause: rustup's **default** toolchain here
is `stable-x86_64-pc-windows-gnu`, and msvc is active only through a
*directory override* on the repo path - which a `git worktree` at another
path does not inherit. Fix is one environment variable on the worktree build,
no global change:

```
RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc cargo build --release
```

#### Measured: pre-Phase-0 (52a4a6c) vs Phase 0, release, min of 7

| Benchmark | Baseline | Phase 0 | Change |
|---|---|---|---|
| map | 624 ms | 499 ms | **+20.0%** |
| oop | 544 ms | 508 ms | **+6.6%** |
| loop_arith | 666 ms | 642 ms | +3.6% |
| array | 413 ms | 402 ms | +2.7% |
| fib | 375 ms | 372 ms | +0.8% |
| sort | 923 ms | 916 ms | +0.8% |
| mandel | 528 ms | 526 ms | +0.4% |
| closure | 347 ms | 347 ms | 0.0% |
| startup | 91 ms | 91 ms | 0.0% |
| strings | 228 ms | 308 ms | **-35%** |

`oop` improving is the expected payoff from 0.2/0.3 - instances stopped
carrying a hash map each. `startup` did not move, which §24.5 asks to be
watched.

#### Open: the `strings` regression

Reproducible, ~35%, and **not yet root-caused**. Investigated and ruled out:

- not noise - stable across 12 interleaved runs of each binary
- not the `Value::VmFunction` variant - reverting to the fat `Rc<dyn Trait>`
  is equally slow (and that fat pointer was itself a bug: it pushed `Value`
  from 16 to 24 bytes, invalidating §4.1's premise; now fixed and pinned by
  `crates/saule-interpreter/tests/value_size.rs`)
- not LTO unit composition - removing the new `saule-vm` dependency from
  `saule-cli` (release builds use `lto = "fat"`, `codegen-units = 1`) does not
  recover it
- not call overhead - a *user function* call in the same loop is unaffected
- not concatenation alone, and not a native call alone - each in isolation is
  unchanged

Minimal reproducer: a loop body that concatenates into a `local` **and** has
a second live `Value::Str` in the same iteration. Even
`local s = "a" .. i .. "b"` followed by `local t = s` - no call at all - is
30% slower. One live string is fine; two is not.

Prime remaining suspect: `Spanned<Expr>` grew from 88 to 96 bytes when it
gained `NodeId` (four bytes of id plus four of padding, since `Range<usize>`
forces 8-byte alignment). That is the only Phase 0 change on the tree-walker's
hot path, and it fits the pattern - benchmarks that gain something from Phase 0
improve, and `strings`, which uses no classes, enums or closures, gets the cost
with none of the benefit. Not proven. `crates/saule-ast/tests/node_size.rs`
pins the size so it cannot grow again unnoticed.

Not treated as a blocker: it is one benchmark, ~80 ms, on the evaluator the VM
exists to replace.

# Phase 1 — Crate skeleton

*Estimate: 1 week. **Complete.***

- [x] `crates/saule-vm/` created; depends on `saule-interpreter`, never the
      reverse (§22.1)
- [x] `op.rs` — all 115 opcodes from §15 (116 since §19 appended `VARARG`), `Fmt`, `Instruction` encode/decode
      for ABC / ABx / AsBx / Ax, round-trip tests over the full `sBx` range
- [x] `chunk.rs` — `Chunk`, `Proto`, `ClassProto`, `EnumProto`,
      `FieldLayout`, `Handler`, `LineEntry`, `InlineCache`, `JumpTable`,
      `TypeDesc`, `UpvalDesc`, plus `Proto::span_at` binary search
- [x] `disasm.rs` — written *before* the compiler, on purpose. Driven by
      `Op::fmt()`, so a new opcode prints correctly with no change here.
      Resolves jump displacements to absolute targets and spells out
      constants, closures, and switch tables.
- [x] `vm/` — frames, open/closed upvalues, the dispatch loop
- [x] `Value::VmFunction` + the `VmFunction` trait in `saule-interpreter`, so
      a compiled closure can sit in a register without inverting the
      dependency
- [x] Hand-assembled chunks execute: `1+2`, Appendix B.1's sum loop,
      recursive `fib`, upvalue capture, tables, `CONCAT`, error paths
- [x] `saule disasm <file>` wired into the CLI. The whole path — lex, parse,
      semantic, typeck, compile, disassemble — is live; a construct codegen
      cannot emit yet is reported by name with a span rather than silently
      omitted, which is the same signal `--vm` will use to fall back in
      Phase 2. `saule-cli` is the one binary depending on both engines,
      which is where §22.4 puts engine selection.
- [x] Property tests over instruction encoding
      (`crates/saule-vm/tests/encoding.rs`): every opcode round-trips in its
      own format over random operands, operands never bleed into the opcode
      byte, the **full** `sBx` range round-trips exhaustively (that field is
      what silently truncated a `LOADI` operand during Phase 1), out-of-range
      operands are refused rather than truncated, arbitrary 32-bit words
      decode without panicking, and the opcode numbering is pinned because
      it is the chunk ABI.

**Phase 1 is complete.**

---

# Phase 2 — Core VM

*Estimate: 4–6 weeks. **Language subset complete; exit criteria met.***

### Dispatch loop

- [x] Moves and constants; upvalues; module slots; `CLOSURE`
- [x] Integer arithmetic, wrapping, with `DivisionByZero` and the
      negative-exponent rule matching `int_op` exactly
- [x] `ADDII`/`SUBII`/`MULII` immediate forms
- [x] Float arithmetic; bitwise with Lua 5.3 shift semantics
- [x] Fused comparison-and-branch, `TEST`/`TESTSET`, boolean-producing forms
- [x] `FORPREP_I`/`FORLOOP_I`/`FORPREP_F`/`FORLOOP_F`, including the
      overflow guard `run_numeric_loop_int` has
- [x] Tables: `NEWT`, `SETLIST`, `GETARR`/`SETARR`, `GETMAP`/`SETMAP`,
      `GETMAPK`/`SETMAPK`, `APPEND`, `LEN`
- [x] `CALL` / `CALLK` / `CALLNAT` / `RET` / `RET0` / `RET1`, arguments
      constructed in place in the callee's window (§6.2)
- [x] Natives receive `&stack[base..base+nargs]` — a borrow, no `Vec`, no
      per-argument clone (§13)
- [x] Open/closed upvalues with `CLOSEUP`, giving per-iteration capture for
      free (§7.2)
- [x] `CONCAT` n-ary and single-allocation
- [x] `TAILCALL` — reuse the frame; closes the gap `PRODUCTION.md:344` names.
      **The note that used to sit here had the polarity backwards**, and it
      is worth keeping the correction visible: it argued the VM must *not*
      get tail calls because doing so would create a divergence. That was
      true only while neither engine had them. The tree-walker got a
      trampoline first, and from that moment the divergence existed the
      other way round — `countdown(100000, 0)` returned `5000050000` under
      the tree-walker and `stack overflow` under `--vm`, exit 1 against
      exit 0. Implementing it **closed** a divergence. See "Tail calls"
      below.
- [x] Variadic **return** through `top` (`C = 0` on the call, `B = 0` on the
      `RET`) exercised by tests — see "Multi-return and parallel binding".
      `B = 0` on a *call* is still unused, and deliberately: Saule's
      `eval_call_args` does not expand a trailing call into several
      arguments, so `f(g())` passes exactly one. Implementing it would be
      inventing a language rule, not matching one.
- [x] `SAULE_MAX_DEPTH` — the env override works in both engines, and the
      VM's cap is now **equal to** the tree-walker's `MAX_EVAL_DEPTH`.
      *Deviation from §6.4, argued:* it was set to `1_000_000` on that
      section's reasoning, and the reasoning is sound in isolation — a call
      here is a `Vec` push, not a native frame. But a limit is observable:
      `depth(50_000)` returned `50000` under `--vm` and raised
      `StackOverflow` without it, and "works with `--vm`, crashes without
      it" is the exact surprise the silent fallback exists to prevent. The
      raise and the "frames" re-documentation both move to Phase 4, where
      they become an announced improvement instead of a disagreement.
      Pinned by `deep_recursion_hits_the_same_limit_under_both_engines`.

### Compiler

- [x] `compile/regalloc.rs` — stack discipline, locals pinned for their
      lexical extent, temporaries released LIFO via a `#[must_use]` `Mark`,
      `max_regs` high-water mark (18). Sibling blocks share registers.
      Overflow past 256 is a clean `TooManyRegisters`, never a panic.
- [x] `compile/ctx.rs` — emission, the line table, jump labels and patching,
      the constant pool, the function stack, and the capture walk
- [x] `compile/expr.rs` — literals (`LOADI` vs `LOADK` by range), identifiers
      classified by the `ResolveTable`, integer/float arithmetic and bitwise
      selected from the `TypeTable`, comparisons, `not`, n-ary `CONCAT`,
      short-circuit `and`/`or`/`??`, table literals, indexing, lambdas, calls
- [x] `compile/stmt.rs` — `local`, assignment (name and index), compound
      assignment, `if`/`elseif`/`else`, `while`, `repeat`, numeric `for`
      (int and float), `break`/`continue`, `return` (none/one/many),
      top-level `fn`
- [x] Calls in three forms: `CALLNAT` with the prelude value resolved to a
      constant **at compile time**, `CALLK` for a statically-known top-level
      `fn`, and the generic `CALL` for a value. Arguments are evaluated
      directly into the callee's future frame, so a call copies nothing.
- [x] Closures: `CLOSURE` with upvalue descriptors built by the Lua capture
      walk. The resolver says *which* names a closure captures; the compiler
      assigns the registers, and marks the owning block so `leave_scope`
      asks for a `CLOSEUP`.
- [x] Forward references — proto indices reserved in a pre-pass, so
      `fn a() b() end` above `fn b()` compiles
- [x] `compile/verify.rs` — Pass 4. Register indices within the frame, jumps
      in range and on an instruction boundary, constant/proto/module/upvalue
      indices valid, `EXTRAARG` present where required, no proto that can run
      off its end. Wired into `compile` under `debug_assertions`, so all 43
      differential tests compile through it.
- [x] Peephole during emission (drop `MOVE r,r`, fold small `LOADK` into the
      `*II` immediates, fuse comparison + branch, drop jumps to the next
      instruction). Deferred here because each is a measurable optimisation
      and §16 says to measure first — **and that is what eventually happened**:
      `--profile-bytecode` landed in Phase 5 and then chose these four ahead
      of every candidate that phase was written around. Three of them turned
      out to be opcodes the VM already had and the compiler never emitted.
      Done in Phase 5; see "§17 emission peepholes" there for the numbers.
      Only `MOVE r,r` was not implemented — nothing appears to emit one, so
      it wants a debug assertion rather than a peephole.

### Integration

- [x] `--vm` flag on `saule run`, and `SAULE_ENGINE=vm`
- [x] Anything the compiler cannot handle returns `CompileError::Unsupported`
      and the CLI **falls back to the tree-walker with a note**, so `--vm` is
      safe to pass on any program. Every other compile error is surfaced.
- [x] All 235 fixtures pass under `SAULE_ENGINE=vm` (via fallback where the
      compiler does not reach yet). This regressed to 9 failures during
      Phase 3 and is green again — see "Nine fixtures the VM ran wrongly"
      below for what broke and why. **Run this alongside the default-engine
      suite; the ground rule only covers the latter, and every one of those
      nine was invisible to it.**

### Deferred out of Phase 2

- [x] `TAILCALL` — done, and not by choice of timing: the tree-walker
      acquired a trampoline, which made this a live divergence rather than a
      future feature. See "Tail calls" below.
- [x] Variadic call/return through `top` — done on the return side; the
      argument side has no language rule to implement (see above)
- [x] `SAULE_MAX_DEPTH` aligned with the tree-walker; the "frames"
      re-documentation and the raise move to Phase 4 (see above)

### Phase 2 exit criteria

- [x] Programs produce **identical results** under both engines —
      `crates/saule-vm/tests/differential.rs`, 43 tests at the time (99
      now), comparing values *and error text*
- [x] **`loop_arith` and `fib` at least 2.5x faster than the tree-walker**

Measured with `cargo run --release -p saule-vm --example compare`, which
also asserts the two engines agree before timing:

| Program | Tree-walker | VM | Speedup |
|---|---|---|---|
| fib | 189 ms | 52 ms | **3.6x** |
| call_heavy | 336 ms | 87 ms | **3.9x** |
| loop_arith | 395 ms | 158 ms | **2.5x** |
| while_sum | 435 ms | 180 ms | 2.4x |

`fib` clears the bar comfortably. `loop_arith` sits **right on it** —
2.49x–2.55x across runs — so it passes, but with no margin. Worth knowing
before Phase 5: none of the optimisations that would widen it have been
applied yet (no `get_unchecked`, no superinstructions, no `*II` immediate
folding, and a tag check on every typed opcode).

A note on the benchmark files: `benchmarks/sau/*.sau` all wrap their work in
`class Main` with a `static fn main()`, and classes are Phase 3 — so the
files themselves still fall back to the tree-walker. The programs measured
above are the same shapes without that wrapper. Running the real benchmark
files under the VM is the first thing Phase 3 unlocks.

---

# Phase 3 — Full language

*Estimate: 4–6 weeks. **Done.** In dependency order. Read the exit criteria at
the end of this phase before the item list — several boxes below stayed `[ ]`
long after they were true, and one stayed `[x]` while its claim was false.*

1. **Classes** — §8. **Done.** The one `[ ]` left below is §8.5's inline
   cache, which is Phase 5 and performance rather than coverage. (This line
   said "except the two items marked `[ ]` at the end"; there is one, and it
   is not at the end.)
   - [x] **Pass 1 layout** (`compile/layout.rs`): field slots, vtable slots,
         statics index and the `init` slot per class. Field slots **and**
         vtable slots extend the parent's rather than reordering them, which
         is what makes a slot resolved against a static type correct for any
         subclass. Classes are ordered by inheritance depth, so a subclass
         declared above its parent still lays out correctly. Matches the
         tree-walker's rule that a defaulted field becomes a *static* when
         the class has no `init`. 8 tests.
   - [x] **§24.2 single-sourcing, structurally.** `chunk.rs` had defined its
         own `FieldLayout` next to the one in `saule-interpreter` — two
         definitions, two chances to diverge, which is precisely the failure
         §24.2 names. The copy is deleted; `saule-vm` re-exports the
         interpreter's type and builds layouts through the same
         `FieldLayout::build` the tree-walker uses, and the runtime
         `ClassObject` holds the very same `Rc`. Divergence is now
         unrepresentable rather than merely unlikely.
   - [x] **VM opcodes**: `NEW`, `GETF`/`SETF`, `GETSTAT`/`SETSTAT`, `ISA`,
         `CALLM` (vtable dispatch, receiver as parameter 0 so the argument
         window needs no shuffling), `CALLSTAT`. Runtime `ClassObject`s are
         built once at start-up from the protos, parents first.
   - [x] **Codegen** (`compile/class.rs`): class declarations, method bodies
         with `self` as parameter 0, `ClassName(args)` construction,
         `GETF`/`SETF` member access, `CALLM` instance dispatch, `CALLSTAT`,
         static fields, field defaults via a synthetic `field_init` proto
         run parent-first, and `self.super()`
   - [x] `self.super()` dispatches **statically** to the parent's `init`.
         Going through the vtable would re-enter the child's own constructor
         and recurse forever, since the child overrode that slot.
   - [x] Construction calls `init` on a *copy* of the instance: `CALLM`
         writes its result over the receiver, and `init` returns nil, which
         would otherwise discard the object being built.
   - [x] Stdlib statics (`String.len`, `Table.insert`) resolve to their
         native at compile time and compile to `CALLNAT` — the prelude is
         fixed before a program runs, so nothing looks them up at run time
   - [x] `saule_vm::run_chunk_entry` invokes `Main.main()` after the module
         body. Running the body only *declares* the class; without this the
         VM ran every benchmark in ~0 ms because it never started them.
   - [x] **All 10 benchmark files run under the VM**, with output verified
         identical to the interpreter. `sort.sau` was the last holdout and
         landed with re-entrancy.
   - [x] A class from another module is refused rather than guessed — the
         imports slice lifted that for *layout*, and §19 argument binding
         followed later; see "Named arguments across a module boundary".
   - [ ] `CALLM` currently costs one hash probe to map a receiver's class to
         its vtable; §8.5's inline cache is the fix, and it is Phase 5
   - [x] Reading a class-level **static through an instance** (`b.label`,
         `b?.label`). A defaulted field on a class with no `init` *is* a
         static in both engines, so this is the common shape, not an edge
         case. Refused when a subclass redeclares the name, since then the
         answer depends on the receiver's runtime class rather than its
         declared one.
   - [x] **Inherited statics addressed the wrong cell.** Static storage is
         one `Vec<Value>` per class *index*, and `sindex` was copied from
         the parent with the slot numbers kept — so `Derived.total`
         resolved against `Derived` and read a second, never-initialized
         cell: `nil` where the tree-walker gives the parent's value. Found
         while wiring instance-static reads; a silent wrong read, which is
         the failure §24.2 calls the worst this project could ship.
         `sindex` now maps to a `StaticSlot { class, slot }` naming the
         **declaring** class, mirroring
         `ClassObject::declaring_static_field`. Carrying the owner in the
         index is what makes it impossible for a call site to forget.
   - [x] `sort.sau` compiles. It fell back on `Table.sort`'s comparator,
         not on `!` — see "The tree-walker can now call into bytecode".
   - [x] Instance methods on a receiver whose class the front end did not
         prove — `CALLMX` and `GETFX` landed and this works. Verified: a
         `Greeter()` laundered through `any` calls `hi()` identically under
         both engines, and `a_method_call_on_an_unproved_receiver_matches`
         pins it.
   - [x] **Inherited vtable slots were never filled.** Pass 1 copies the
         parent's vtable so the slot *numbering* extends it, but at that
         point no body is compiled, so it copies a row of `u32::MAX` — and
         `class_decl` fills only the slots a class declares itself. An
         inherited, non-overridden method dispatched into nothing:
         "`Circle` has no method in vtable slot 2". Fixed by Pass 2a in
         `compile/mod.rs`, one forward sweep after codegen, parents before
         children.

#### Measured: real benchmark files, interpreter vs `--vm`

Release build, wall clock, min of 3, including ~90 ms of process start-up in
both columns. Output verified identical by `SAULE_DIFF=1 ./run_tests.sh`.

Re-measured after re-entrancy landed, so **all ten compile** — `sort` is a
real engine comparison for the first time. Absolute numbers run higher than
the previous table across the board (same machine, different load); read the
ratios, not the milliseconds.

| Benchmark | Interpreter | VM | Speedup |
|---|---|---|---|
| mandel | 583 ms | 255 ms | 2.29x |
| closure | 395 ms | 181 ms | 2.18x |
| loop_arith | 762 ms | 349 ms | 2.18x |
| fib | 424 ms | 196 ms | 2.16x |
| oop | 573 ms | 269 ms | 2.13x |
| array | 461 ms | 227 ms | 2.03x |
| strings | 345 ms | 187 ms | 1.84x |
| map | 607 ms | 487 ms | 1.25x |
| sort | 1000 ms | 881 ms | **1.14x** |
| startup | 105 ms | 103 ms | 1.02x |

Net of the ~100 ms of process start-up in both columns, the ratios are
roughly 2.2x–3.4x.

Two of these are still not really engine measurements:

* **`startup`** does no work; it measures process start-up, which the VM
  neither helps nor hurts. §24.5 asks for it to be watched, and it has not
  moved.
* **`map`** is dominated by hashing inside `TableObject`, which the VM does
  not change — exactly as §20 predicts.

**`sort` needed a fix beyond making it compile.** On the first run it was
*slower* than the tree-walker — 1146 ms against 1092 — even though the
comparator body itself is faster. The comparator crosses the engine boundary
once per comparison, and each crossing built a fresh `Vm`: two heap
allocations, a 256-register stack and a frame list, `n log n` times. A
bounded free list of parked register files on `VmShared` (`take_vm` /
`give_vm`) removed them and took it to 881 ms. Only a cleanly-returned `Vm`
is parked; one unwound by an error still has frames and is dropped.

Worth remembering as a general point about re-entrancy: crossing the
boundary is not free, and a benchmark that newly *compiles* is not
automatically a benchmark that got *faster*.
2. **Interfaces** — [x] itables, `CALLIF`. Per class, one table per
   implemented interface mapping the interface's method slot to that class's
   vtable slot, built once at layout time — so a call is a small-map probe
   and two indexed loads, never a name lookup. `extends` between interfaces
   is flattened into a single slot list, so an implementing class needs one
   itable per interface rather than one per level.
   [ ] The one-entry inline cache (§8.4) is Phase 5: interface call sites are
   overwhelmingly monomorphic, so it collapses the probe to a pointer
   compare — but that wants a benchmark, not a guess.
3. **Enums and `match`** — [x] dense tags, `GETTAG`, `SWITCH` + jump tables,
   `VARIANT`/`NEWVAR`/`UNWRAP`/`JIFTAG`, pattern compilation, guards,
   payload destructuring, `.value` on a valued variant.
   When every arm is a distinct variant of one enum with no guards — the
   dominant shape — it compiles to `GETTAG` + `SWITCH`: **O(1) instead of
   O(arms)**, with no string compared, where the tree-walker compares both
   the enum name and the variant name once per arm. Anything else falls back
   to a test chain, so correctness never depends on the jump table firing.
   A trailing wildcard becomes the table's default.
   [ ] A valued variant's value must be a *literal*: a chunk stores
   constants, not code. Non-literals are refused rather than mis-evaluated.
   [x] Tuple patterns (`case (q, r)`) and nested payload patterns — see
   "Tuple patterns and nested payload patterns" below.
4. **`try`/`catch`/`throw`** — [x] handler tables, unwinding, `CHKTY`,
   `TypeDesc` runtime type tests.
   Entering a `try` emits **zero instructions**: the protected range lives
   out of band in the proto's handler table and only a `throw` consults it,
   so the happy path costs nothing.
   The thrown value never enters a `RuntimeError` unless it escapes to the
   top level, which is why the VM needs no equivalent of the tree-walker's
   `thrown_slot` thread-local (§12.1).
   Unwinding closes upvalues at each frame it leaves, so a closure built
   inside the `try` cannot outlive its registers.
5. **`for … in`** — [x] the table snapshot (array then *sorted* map entries
   — that ordering is observable and must be preserved) via
   `ITERPREP`/`ITERNEXT`, and [x] the §15.8 **closure driver**, including
   `iter()` on instances.

   The driver is **not** taught to `ITERNEXT`. It lowers to an ordinary
   `CALL` in a `while` shape, because `CALL` already dispatches on whatever
   it finds — a bytecode closure, a native, a native closure — so a driver
   can be any of them with no new opcode and no new VM path. Teaching
   `ITERNEXT` to call would have meant pushing a frame from inside an opcode
   and resuming into it, which is the dispatch loop's hardest corner, for no
   gain. An instance source gets one `CALLM` to `iter()` first.

   **The result count is fixed at `nvars`, not variadic**, and that is the
   part worth remembering. `C = nvars + 1` asks for exactly as many values
   as there are loop variables, so `pop_frame` pads the short cases with
   `nil` and drops the surplus — which is exactly `exec_for_in`'s "extras →
   nil, surplus dropped", for free. Asking for *all* results instead would
   leave the callee register holding the driver itself when a step returned
   nothing — a function, not nil — and the loop would never terminate.
   Pinned by `a_driver_that_yields_nothing_runs_no_iterations`.

   - [x] A source the front end **did not prove** — `ITERPREPX`. This is
         the shape real code hits: `todo-app` iterates an `any` that came
         out of `Json.decode` behind a runtime `type(data) != "table"`
         guard, which no static type can see through.

         **The trap was settled by rejecting its premise.** The tempting
         fix was an `ITERDRV` normalising every source into one driver, so
         a single lowering serves everything. It cannot work, and the
         reason is semantic rather than incidental: a driver stops on a
         `nil` and a table snapshot has *no* terminator at all. Saule's
         `t[i] = nil` **stores** a nil rather than deleting the key (unlike
         Lua — see `TableObject::set`), so a table really can hold one, and
         a one-variable loop binds the **value**. Measured, not reasoned
         about: `{1, 2, 3}` with `t[2] = nil` iterates **3** times under
         the tree-walker and would stop at **1** under a normalising
         driver; with `t[1] = nil` it would stop at **0**.

         So `ITERPREPX` **dispatches** instead — which is exactly what
         `exec_for_in`'s `match` on the source value does. It writes a mode
         flag to `R[A+2]`, and the compiler emits *both* steps behind a
         `TEST`: table mode falls straight through to the existing
         `ITERNEXT`, driver mode takes one jump to a `MOVE`/`CALL`/`JNOTNIL`
         sequence. One loop body, one set of variable registers, and no new
         VM call path — the driver step is an ordinary `CALL`, as §15.8
         already required.

         Two details worth keeping:
         - The driver's **call window is placed on the loop-variable
           registers** rather than moved into them — `R[A+4]` for one
           variable, `R[A+3]` for two — so `CALL` writes its results exactly
           where `ITERNEXT` writes its key and value. No `MOVE`s to merge
           the paths, and the nil test lands on the first returned value,
           which is what the tree-walker tests.
         - An **instance** source calls `iter()` inside the opcode, via
           `saule_interpreter::call_member_dynamic` — the same function the
           tree-walker uses. That is a re-entrant call from inside the
           dispatch loop, which §15.8 rejected for `ITERNEXT`, and the
           objection does not apply here: `iter()` runs **once per loop**,
           not once per step.

         **`Value::VmFunction` is the bug this shipped with for an hour.**
         The callable test was copied from `exec_for_in`, which lists
         `Function`/`Native`/`NativeClosure` — and never `VmFunction`,
         because the tree-walker cannot construct one. Under the VM a
         compiled closure *is* a `VmFunction`, so every driver on this path
         was refused as "cannot iterate over a `function`". A place where
         copying the oracle verbatim is wrong precisely because the two
         engines represent the same value differently.
6. **Operator overloading** — [x] compile-time contract resolution via
   `binary_contract`; dispatch-on-left-operand and the `==`/`compare`
   symmetry rules moved into the compiler, including unary and index.

   **This box read `[ ]` long after it was true** — the code is in
   `binary_to`, resolved through `saule_ast::ops::binary_contract` against the
   *left* operand's proved class, with `equals` normalised through two `NOT`s
   and `compare` read against a `LOADI 0`. The summary at the top of this
   file had it right and the checkbox did not, which is the same drift the
   "Where things stand" paragraph carried in the other direction.

   One comment beside it is now stale in its *reasoning* though not in its
   conclusion: it says the overload "must" be resolved at compile time
   because "the runtime `ClassObject` the VM builds has an empty method map".
   That stopped being true when `MethodRef` landed. Compile-time resolution
   is still right — it costs nothing at run time — but it is now a choice
   rather than a necessity.
7. **Nullability** — [x] `?.`, `??`, `!`, `as`.
   `??` already compiled (`JNIL` + `JMP`, laziness preserved). Added: `!`
   → `UNWRAPNIL`; `x as T` → `CASTCHK`; `obj?.name` and `obj?.method(args)`
   → `JNOTNIL` around the access, with a `LOADNIL` on the nil arm.

   **`CASTCHK` carries a `Type`, not a `TypeDesc`.** `catch` filters on a
   shallow test; `x as T` is *deep* — `t as table<integer>` walks every
   element, `x as Animal` walks the inheritance chain — and a `TypeDesc`
   cannot express that. So `Chunk::cast_types` stores the source
   `saule_ast::Type` and the opcode calls
   `saule_interpreter::eval::expr::cast::cast` **directly**, the same
   reuse-rather-than-reimplement move `ARITHX` makes with `ops::binary`.
   Divergence is unrepresentable rather than merely unlikely.
   *Deviation from §15.12:* the design says "type descriptor in `K[C]`";
   `C` indexes `cast_types` instead. Types are interned, so a program casts
   to a handful of distinct types however often it writes `as`; past 256 it
   is a clean `Unsupported`.

   `obj?.method(args)` guards the **whole call**, arguments included — the
   tree-walker returns before evaluating them, so evaluating them here
   would run side effects it does not. Asserted by a differential test that
   counts them.

   - [x] `?.` on a receiver whose class the front end did not prove, and on
         a table receiver. **This box read `[ ]` long after it was true** —
         the dynamic safe read and safe call landed with `GETFX`/`SETFX` in
         the correctness slice (see "The open correctness items — closed"),
         not in Phase 5. Verified rather than assumed: `local t = {a: 1}`
         then `t?.a`, and `t.g?.hi()` on a receiver the front end never
         proved, both compile and both agree with the tree-walker. A
         *string* receiver is a type error in **both** engines (`cannot read
         field ... on value of type string`), so it is a language rule, not a
         VM gap.
   - [x] `Nullable` `catch` types. **This was not a gap, it was a live
         silent divergence** — see "A nullable `catch` type caught
         everything" below. `TypeDesc` grew a `Nullable(u32)` pointing into
         the same descriptor pool, and `value_matches` reads it as
         `nil || inner`, which is what `runtime_matches_type` does.
   - [x] Tuple `catch` types still collapse to `TypeDesc::Any`, and that is
         **correct**: the oracle's own arm is `Type::Tuple(_) => true`
         (`multi-return shapes aren't introspectable here`). Checked rather
         than assumed — the neighbouring `Nullable` case in the same line of
         this file was wrong, which is why this one was verified against
         `try_.rs` instead of reasoned about.

   **Front-end fix this needed:** `saule-typeck`'s `Expr::Call` arm
   dispatches on the *shape* of the callee rather than walking into it, so
   the receiver of `obj?.method(args)` was the one receiver position
   nothing ever inferred — and an uninferred node has no `TypeTable` entry,
   which left the compiler unable to resolve the vtable slot. `check_expr`
   now infers it for the recording side effect. No diagnostic is produced,
   so `check` and `check_with_types` still agree byte for byte.
   - [ ] *Noticed, not fixed:* the same gap means a safe method call's
         **arguments are never type-checked**. `g?.twice("no")` passes
         typeck today. Mirroring the `Member` branch would fix it, but that
         adds diagnostics to a working language and belongs in its own
         change.
8. **Stdlib value members and table dot access** — [x]
   - `Math.pi`, `Os.sep`, `IoMode.Write` fold to a `LOADK` at compile time.
     The prelude is fixed before a program runs, so this is the same
     resolution `String.len` already got on the call path, applied to
     members that hold a value rather than a function.
   - `t.foo` on a proved table is `t["foo"]` — `GETMAPK`/`SETMAPK`. Past a
     constant index of 255 the key is materialised and `GETIDX`/`SETIDX`
     does the work, because capping a module at 256 constants on an
     operation as ordinary as `t.name` is a worse failure than one extra
     instruction.
   - Not folded when the module **writes** through the receiver:
     `Math.pi = 3.0` is accepted today (the typechecker does not reject
     it), so a fold would freeze a value the program then changes. Needed
     `saule_ast::visit`'s new `Visitor::assign_target` — a flattened
     expression walk cannot tell a read from a write.

   **Two shadowing bugs found here, one of them pre-existing.** A
   module-level `local` becomes a module *slot*, not a frame local, so
   `FuncCtx::lookup` structurally cannot see it — and every "is this bare
   name really the class / enum / stdlib entity it looks like?" test was
   built on that lookup alone. So:
   - `local Math = {pi: 3.0}` then `Math.pi` read **π** (my fold), and
   - `local String = {...}` then `String.len("abc")` called the **stdlib's**
     `String.len` (pre-existing, since the `CALLNAT` path landed), and
   - `local Foo = {...}` shadowing a class `Foo` read the **class's** static.

   All three now go through `Compiler::not_shadowed`, which checks both
   places a `local` can land, or through the resolver's `Binding::Prelude`
   where that is the precise question. Three differential tests pin them.

9. **Pipes** — [x] `when(source):a(x):b(y)` lowers to a chain of ordinary
   calls, each threading the upstream value in as argument 0 — what `eval`'s
   `Expr::Pipe` arm does. The value lives in one register for the whole
   chain and every stage writes its result back there.

   **The stage callee is resolved by name, not through the binding table.**
   A `PipeStage` holds a bare `String` and has no `NodeId`, so `Bindings`
   has nothing keyed on it — the same shape as the enum-method gap in §0.6.
   The lookup order is therefore written out by hand and has to match the
   resolver's: a local shadows a top-level `fn`, which shadows a module
   slot. Getting that order wrong is the `local String = {…}` bug this
   compiler has already shipped once.

   Two shapes deliberately have no branch, because `saule-typeck` rejects
   them before the compiler sees them: a **prelude** name as a stage
   (`when(x):tostring()` is `UnknownPipeStage`) and a locally-bound lambda.
   Writing code for either would be unreachable branches pretending to be
   features.
10. **Imports and modules** — [x] **done.** (Was `[~] in progress` long
    after the last sub-item was checked off; nothing under it is open.)
    Decision taken: a
    **program-global class table** with **per-module chunks**, which
    satisfies §14 (per-module chunks keep a bytecode cache possible) and
    §24.2 (one layout, not two). The alternative — folding every module into
    one chunk — is simpler but would have precluded per-module caching.

    - [x] `Chunk::classes` / `enums` / `interfaces` are `Rc<Vec<_>>`, shared
          by every module of a program. Mutated during compilation through
          `classes_mut()`, which is `Rc::get_mut` and **not** `make_mut`:
          `make_mut` would silently clone the table if anyone else held it,
          leaving the compiler writing vtable slots into a copy the VM never
          sees — §24.2's failure with no symptom.
    - [x] `program.rs`: `load_units` walks the import graph from the entry
          file, reading and parsing each module, and returns them in
          **post-order** — every module after the ones it imports. Post-order
          is not just a compile requirement (a parent must be laid out before
          its subclass); it is observable, because the tree-walker runs an
          imported module's top level on first import.
    - [x] Import cycles refuse rather than loop. A native-package import
          refuses too — there is no Saule module to compile, and *skipping*
          it would leave the names silently bound to `nil`.
    - [x] `layout::build` appends into the program's table and takes an
          `imported: &Layouts`, so a parent from another module resolves to
          the index its own module assigned. The prefix invariant holds
          across the boundary — asserted by
          `an_imported_parent_resolves_to_the_index_its_own_module_assigned`.
          The `a class extending one from another module` refusal is gone;
          what remains refuses only a parent that is *nowhere*.
    - [x] `program::compile`: the per-module codegen loop. The front end runs
          **per module** — `bindings` and `types` cannot be shared, because
          `NodeId`s are per module and every module numbers from zero, so one
          module's `TypeTable` entry would answer another module's question.
          The type tables, by contrast, accumulate: `compile::Tables` is
          *moved* into each chunk and back out, which keeps the refcount at
          one so `classes_mut`'s `Rc::get_mut` never fails, and only becomes
          a shared `Rc` after the last module. Asserted by `Rc::ptr_eq`
          across two chunks — two equal-*looking* tables would still be
          §24.2 waiting to happen.
    - [x] An `import` whose names a driver already bound emits **zero
          instructions**: a type is a compile-time index, so there is nothing
          to do at run time. Without a driver it still refuses, because the
          name has a module slot nothing would write. One flag,
          `Compiler::imports_bound`, distinguishes them.
    - [x] Pass 2a's vtable-inheritance sweep is restricted to the module
          being compiled. It still *reads* the whole table, so a child here
          with a parent over there resolves; it just does not revisit rows an
          earlier module already finished.
    - [x] Imported **values** need no new opcode. Every module's slots are
          rebased at compile time onto one flat program-wide vector, so the
          exporter's slot and the importer's are two indices into the same
          array and the copy is an ordinary `GETMOD` + `SETMOD` prologue.
          That fell out of `Bx` being 16 bits: 65 536 top-level names across
          a program, refused cleanly past that. Imported **types** need not
          even that — they are compile-time indices.
    - [x] VM: `Vm::for_chunks` builds **one** `VmShared` for the program.
          Per-module would give each module its own `Rc<ClassObject>` for
          the same class, and `class_of` — which maps class identity to a
          vtable — would then answer differently depending on who asked.
    - [x] `Closure` carries its `Rc<Chunk>`. Constants, protos, jump tables
          and cast types are indexed **per chunk**, so they follow the frame:
          a closure built in one module and called from another must read its
          own module's pools. Classes, enums and interfaces are the exception
          — every chunk shares those `Rc`s, so reading them through whichever
          chunk is running is the same table by construction.
    - [x] `ClassProto` and `EnumProto` record their declaring module.
          `vtable` and `static_methods` hold proto indices and a variant's
          value is a constant index; both are per chunk.
    - [x] **`CALLK` now carries its module**, packed 8/16 in `EXTRAARG`.
          Found by running a real two-module program: `self.super()` on a
          parent from another module loaded the *running* module's proto of
          the same number — which for a subclass's `init` was its own `init`.
          Unbounded recursion, and only because the two happened to be
          numbered alike. Nothing in the fixture suite could have caught it;
          it took an actual cross-module program.
    - [x] CLI wired to `program::compile`. A module it cannot read, resolve
          or order falls back with a note rather than failing the run — the
          tree-walker resolves imports its own way and must stay the oracle
          for whether a program is valid.
    - [x] **Native-package imports fold at compile time.** A native package
          is Rust-built values with no Saule source, so there is nothing to
          compile and nothing to run — its exports are fixed before the
          program starts, exactly like the prelude. `Compiler::static_value`
          now answers for both, and the four `Binding::Prelude` gates were
          widened to go through it.
    - [x] A **dynamic** (manifest-described, `dlopen`-ed) package still
          refuses. Loading one is a runtime side effect, and compiling must
          not perform it.
    - [x] `differential.rs`'s unsupported-construct canary re-pointed at a
          pipe. A lone `import` compiled through the single-module path
          still refuses, and that is now pinned by its own test
          (`an_import_without_a_program_driver_still_refuses`) — it is a
          correctness rule, not a stand-in.

#### Measured: example projects

**Correcting an earlier measurement in this file.** The "entry points" run
recorded here previously ran `main.sau` files *directly*, which is not how a
Saule project is invoked: `json` and friends resolve through the project's
`src_dirs`, so a direct-file run fails on **both** engines. Running the
projects is the honest measurement.

| | Before this slice | After |
|---|---|---|
| `examples/*/` projects running fully on the VM | 0 of 11 | **4 of 11** |

Every one previously refused at its first `import`. The eight that remain
fail on other things, and the `match`-guard fix moved two of them onto a
new top cause:

| Cause | Projects |
|---|---|
| an import of a dynamic native package | 2 | 
| a skipped parameter whose default must run in the callee | 1 |
| a variant of an unknown enum | 1 |
| a class implementing `Assignable` | 1 |

Re-censused after the named-argument fix. **The two dynamic-native-package
projects are a deliberate refusal, not a gap** — loading one is a runtime
side effect and compiling must not perform it (§ item 10) — and they are also
the two interactive projects the diff harness skips. So the addressable
remainder is **three projects, three distinct causes**, one of which
(`a skipped parameter whose default must run in the callee`) is the last open
sub-item of item 11.

This table is the one to steer by. Run it with a timeout — two of these
projects open a window and never terminate:

```
while IFS= read -r cfg; do
  timeout 8 env SAULE_ENGINE=vm ./target/debug/saule.exe run "$(dirname "$cfg")" 2>&1 \
    | tr '\n' ' ' | grep -oE 'does not handle `[^`]*`' | head -1
done < <(find examples -name saule.config) | sort | uniq -c | sort -rn
```
11. **Variadics, trailing blocks, named arguments, defaults** — [~] §19.
    One sub-item open: a default skipped in the *middle* of a parameter list
    (see below). Everything else in this item is done.
    Entry stubs, as Q3 recommended — and no microbenchmark was needed to
    choose, because the alternative is not merely slower but *wrong*: a
    default has to be evaluated in the **callee's** frame, and a guarded
    prologue at the call site would resolve `fn f(a, b = a * 2)`'s `a`
    against the caller. Stubs get it right by construction.

    - [x] **Defaults** → per-arity entry stubs. `entries[k]` is where filling
          starts for arity `k`, and the stubs fall through into one another
          and then into the body: entering at `k` fills `k`, `k+1`, … with no
          jumps. A call passing everything enters at the body, so a default
          costs nothing when it is not used.
    - [x] A **method's** stubs are indexed by arity *including `self`*, and
          its parameters start at register 1. Off by one and a one-argument
          call runs the wrong stub — pinned by its own test.
    - [x] **Named arguments** reordered once in `call_to`, into plain
          parameter order, so every path below it (`CALLK`, `CALLSTAT`,
          `CALLM`, the constructor, `CALLNAT`) sees an ordinary positional
          list and the runtime never sees a name. The slot assignment is
          `saule_ast::resolve_arg_slots` — the function the typechecker uses
          — so the two cannot disagree about which parameter an argument
          fills, trailing-block rule included.
    - [x] A named call may **skip** a nullable parameter: the gap is filled
          with a synthesized `nil`, which is what the callee would have left
          there. Reading the callee's parameter *names* needs a
          `callee_params` map collected in a pre-pass, because a `Proto`
          deliberately carries neither names nor defaults — those are
          compile-time facts.
    - [x] A skipped parameter that has a **default**. Two different cases,
          both now handled — the second by materializing a *literal* at the
          call site; see "A default skipped in the middle" below:
          * A default at the **end** of the parameter list, filled by the
            entry stubs, does run in the callee: a default of `nextId()`
            called twice yields `a#1`, `b#2`, `calls=2` under both engines,
            so it is evaluated per call rather than folded once at the call
            site. Pinned by `a_default_is_evaluated_in_the_callees_frame`.
          * A default **skipped in the middle**. Stubs fill a *suffix*, so
            there is no entry point meaning "fill slot 1 but not slot 2". A
            **scalar literal** default is materialized at the call site
            instead; anything else still refuses, now as `a skipped parameter
            whose non-literal default must run in the callee`.

          **This item was once marked `[x]` while claiming
          `trailing_block_layout.sau` compiles, when it did not.** It does
          now, and the claim is re-checked rather than carried forward.
    - [x] **Variadic** parameters, via a new `VARARG` opcode — the first one
          this project has added since the table was frozen in Phase 1, and
          **appended**, never inserted, because the numbering is the chunk
          ABI. `Frame` gained `n_args` to feed it.

          Gathered by the **callee**, as its first instruction, so it runs
          however the frame was entered. The tempting alternative — have the
          *caller* pack a table, needing no new opcode at all — only works
          where the caller can see that the callee is variadic: not through
          a function value, and not across a module boundary, where
          `callee_params` holds only this module's declarations. A callee
          that gathers its own arguments is right for every call.

          A call passing no surplus gets an **empty table**, not nil: the
          parameter is always a table, or `#values` and `for … in` would
          both fault on the zero case.

          *Front-end wrinkle:* `...values: integer` types `values` as
          `integer`, not `table<integer>`, so `for v in values` cannot be
          proved a table from the `TypeTable`. The compiler records the
          variadic parameter's name itself and takes the table path on it —
          it emitted the `VARARG`, so it knows.
    - [x] Both a default **and** a variadic parameter in one signature.
          `fn tally(label: string = "n", ...nums: integer)` compiles and
          `tally("x", 1, 2, 3)` gives `x=6` under both engines.
12. **`ARITHX`/`UNARYX`** — [x] the dynamic fallback.
    Calls `saule_interpreter::eval::ops::binary` / `unary` directly — the
    tree-walker's own operator logic, **reused rather than reimplemented**,
    so string coercion, `Op*` dispatch and every error message are identical
    by construction instead of by care.
    The operator rides in `EXTRAARG` under an **explicit** numbering rather
    than `BinOp`'s discriminants: those are an implementation detail of
    `saule-ast` that a refactor could renumber silently, and this value is
    part of the chunk ABI.
    Arithmetic, comparisons, bitwise ops, concatenation and unary negation
    all fall back rather than refuse when the front end proved nothing —
    which closed the largest remaining source of `Unsupported`: anything
    involving a *call result*. `a.area() + b.area()` now compiles.

### Class statics by bare name — done

Five refusals that looked like five gaps were one area, and closing it took
`tests/*.sau` from 64 to **69 of 92** — more than the six fixtures aimed at,
because several others hit a static somewhere after their first refusal.

- [x] `Binding::ClassStatic` in **read** and **write** position →
      `GETSTAT` / `SETSTAT`. The resolver carries the *class name* rather
      than a slot, because the answer has to survive a lambda nested inside
      the method — a different `FuncCtx` with no `current_class` of its own.
- [x] A `static local fn` called by its bare name from a sibling →
      `CALLSTAT`. It is a *method*, so it lives in `smindex` and not in the
      `sindex` a bare-name static read consults; without its own arm it fell
      through to the generic call and asked for a static field that does not
      exist.
- [x] `self.count` inside a `static fn`. There `self` denotes the **class**
      (`call_static_method_multi` binds it to `Value::Class`), so this is a
      static access resolved at compile time — which is also why the VM
      never needs a class in a register. Bare `self` in a static method is
      still refused, and no fixture asks for it.
- [x] **`smindex` is now flattened, carrying the declaring class.** It was
      the odd one out: `vindex` copies the parent's entries and `sindex`
      carries a `StaticSlot { class, slot }` for exactly this reason, but
      `smindex` held only a class's *own* static methods. So
      `Player.describe()` on a method inherited from `Entity` missed
      entirely and fell back. `static_methods` is one vector per class
      index, so a flattened entry has to name its owner or it would index
      the subclass's own — empty — vector.

*Worth noting:* four different refusal messages (`a class static`,
`assignment to this kind of binding`, `an assignment to something that is
not an instance field`, `` `self` outside a method``) were all the same
area. The fallback-cause histogram groups by *message*, which splits one
cause across four rows and makes each look smaller than it is. Read it as a
starting point, not a ranking.

### §8.5 dynamic member dispatch — done, and what it exposed

`GETFX` and a new `CALLMX` are the escape hatch for a receiver whose class
the front end did not prove — the counterpart of what `ARITHX` did for
operators, and the largest single cause on both corpora before this.

Both **defer to the tree-walker's own member logic**: `GETFX` calls
`read_member`, `CALLMX` calls `dispatch_member_call_multi`. That is what
makes a file handle, an enum variant, a class static and a module-level
variable all work without the compiler learning each one — and what makes
the *error text* match, which a reimplementation would have got wrong first.
`CALLMX` lays its receiver at `A` and arguments after it, exactly as `CALLM`
does, so a call site can pick between the two without moving anything.

The §8.5 inline cache — collapsing the monomorphic case to a slot load — is
still Phase 5, with a benchmark. Correct first.

**Lifting the refusal exposed two bugs that had been hiding behind it**, and
`SAULE_DIFF=1` found both within one run:

1. **`UNWRAP` returned `nil` for a bare variant's `.value`.** The
   tree-walker answers with the variant's own *name* — `Direction.North.value`
   is `"North"`. Pre-existing, and invisible while `enums.sau` fell back on
   the member read that `GETFX` now compiles. A wrong value, exit status 0.
2. **Enum methods.** An enum method has no `NodeId` (§0.6), so the compiler
   cannot produce a proto for it and the runtime `EnumObject` has an empty
   method map. That was harmless while every enum-method call refused on its
   own; `CALLMX` dispatches dynamically, so it reached the empty map and
   reported `no property or method` where the tree-walker succeeds — a
   *failure* where the oracle works. An enum declaring any method now
   refuses the module at layout time.

The second is the more instructive one: a dynamic fallback is only as safe
as the runtime data it falls back *onto*. Widening what compiles turned a
documented, inert gap into a live divergence, and the note in this file that
called it safe ("the compiler refuses an enum method call outright") became
false the moment `CALLMX` landed.

The verifier also earned its keep: `CALLMX` was missing from its
`expect_extra` list, and four fixtures failed with `an EXTRAARG with no
instruction before it` rather than running a mis-decoded chunk.

### `tests/ui/` is the diagnostic corpus — audited

**Every file in `tests/ui/` is a deliberate error.** Each pins a specific
message, and `run_tests.sh` requires all of them to fail. Most are
compile-time — parser, `saule-semantic`, `saule-typeck` — but not all:
`throw_uncaught`, `io_use_after_close`, the `force_unwrap_*` set,
`table_insert_oob`, `pow_negative_exponent` and the two `stack_overflow_*`
fixtures are **runtime** errors, pinned for the same reason. See
`tests/ui/README.md`.

**All 144 fail with a real diagnostic** — none exits 0, none produces an
empty message. But the harness gates on **exit status**, and that is a
weaker check than it looks: a fixture failing for the *wrong* reason passes.
Reading all 144 messages against their filenames found three that were.

| Fixture | What it actually did | Now |
|---|---|---|
| `unknown_field` | Written with `constructor(label)`, which is not Saule syntax, so it died in the **parser** with ``expected `:` and type on field`` and never reached a member check. Its comment still claimed "typeck has no class registry yet", which had long stopped being true. | Rewritten with `fn init`. Fails with ``no member `nonexistentField` on `Box` `` — the language had this right all along and nothing was testing it. |
| `io_use_after_close` | Opened `/tmp/...`, which does not exist on Windows, so `Io.open` returned nil and the `!` killed the run on **line 4** — never reaching `close()`, let alone the use after it. | Relative path. Fails with `type error: file is closed` at the write after the close, which is the point. |
| `match_variant_arity_mismatch` | Fails with `cannot determine the type of this expression` — the generic fallback, not an arity check. The fixture's own comment says "the typechecker should reject this"; it rejects it, but for no stated reason. | **Left as-is and recorded as a gap below.** The fixture is honest about intent; the diagnostic is what is missing. |

*The general lesson, now in `tests/ui/README.md`:* a fixture whose message is
`cannot determine the type of this expression` is a signal rather than a
pass. It usually means the precise check the fixture is named for does not
exist, and the generic fallback is standing in for it.

### Diagnostics worth adding — found by probing, not yet implemented

Constructs the language **accepts silently** or reports late and unhelpfully.
None is a VM/tree-walker divergence; both engines behave identically. Listed
in the order I would fix them.

**Accepted with no diagnostic at all.** Each of these compiles, runs, and
gives the last declaration silently:

| Construct | Today |
|---|---|
| `fn f(a: integer, a: integer)` | accepted; the second `a` wins |
| two methods of the same name in one class | accepted; the second wins |
| two enum variants of the same name | accepted; the second gets its own dense tag, and `match` can only ever reach the first |

The enum one is the most alarming of the three, because it quietly breaks an
invariant the VM relies on: §0.4's tags are dense and assigned in declaration
order, so a duplicate name produces two tags that `by_name` can only map one
way. `SWITCH` then has a jump-table entry nothing can select. Both engines
agree today — they are wrong together — so `SAULE_DIFF=1` cannot see it.

**Reported, but at run time and blaming the wrong thing.** A safe method
call's arguments are never type-checked, which `§21.4 item 7` already
records as noticed-not-fixed. What it costs a user:

```
g?.twice("no")     -- typeck: passes.
                   -- runtime: "arithmetic requires numbers but got
                   --           `string` and `integer`" — blames the `*`
                   --           inside `twice`, not the call site.
g?.nope()          -- runtime: "no method or field `nope`"
g?.twice(1, 2, 3)  -- runtime: "too many arguments: expected 1 but got 2"
```

All three are compile-time facts. The `Expr::Call` arm dispatches on the
*shape* of the callee, and the `SafeMember` branch returns before checking
arguments — the same arm that could not see through an interface receiver
until this session.

**Poor message on a construct that is rejected.** `class C extends A, B`
fails in the parser with ``expected a class member (`[local] name: type`,
`fn`, or `static`)`` pointing at `B`. Correct to reject; the message says
nothing about a class having one parent, and `multiple_extends.sau`'s own
comment ("a class can only extend one parent") is the message a user wants.

**Deliberately not flagged.** `local x` twice in a scope, assigning to a
numeric `for` variable, and dead code after `return` are all accepted, and
all three are accepted by Lua too. They belong to a lint, not to the
typechecker.

### The open correctness items — closed

Six of them, and one of the six turned out to have a front-end half that is
still open. Grouped by what they actually were, rather than by how they were
reported.

#### The dynamic escape hatch had three holes on the nullable and write sides

`GETFX`/`CALLMX` gave an ordinary member read and call a fallback for a
receiver the front end did not prove (§8.5). Three neighbouring cases kept
refusing, and the asymmetry was backwards: **a nullable receiver is if
anything more likely to be unproved than a plain one**, and a write is no
harder to defer than a read.

| Was refused | Now |
|---|---|
| `obj?.name` with no proved class | `GETFX` behind the nil guard |
| `obj?.m()` with no proved class | `CALLMX` behind the nil guard |
| `obj.name = v` with no proved class | **`SETFX`** |

`SETFX` had been in the opcode table since Phase 1 with no body. It calls a
new `saule_interpreter::write_member_dynamic`, which is `assign_member` —
the tree-walker's own member write — exposed the same way `read_member` was
for `GETFX`. An instance field, a class static and a table key are three
different writes, and the compiler learning each one separately is precisely
how the engines diverge.

The nil guard still wraps the **whole** call on the dynamic path, arguments
included, because the tree-walker returns before evaluating them. Counted by
a test rather than assumed.

*Coverage:* `json_usage` was refusing on the write case and now compiles
fully. `todo-app` moved through two refusals to a third.

#### `return x?.m()` — the arms never had to merge

Refused because the nil arm and the call arm produce different numbers of
values, and only the call arm's count is knowable at run time, so there was
no single register run for one `RET` to read.

There does not need to be. **Each arm returns for itself** — the call arm
asks for every result and returns the run `top` delimits, the nil arm returns
a single nil — which is also exactly what the tree-walker does
(`values_of(Value::Nil)` against `dispatch_member_call_multi`). Reported back
through `Results::terminated`, the same contract a tail call already used for
"control has left; emit no `RET`".

#### `return a, f()` — contiguity falls out of the bump allocator

Refused on the grounds that `f`'s window would have to begin exactly where
the fixed values end, "which the allocator cannot promise while `a` is still
live". It can, and it already did: after the first `n - 1` values are in
place, `free` sits precisely at the landing register. Reserving that register
and releasing it again sizes the frame for a single-valued last expression
*and* leaves the next allocation — the call window — landing on it.

**But the front end blocks it**, which the refusal was hiding: `saule-typeck`
models a value list as one value per expression, so `return 7, pair()`
reports `` return value of type `(integer, integer)` is incompatible with
declared return type `integer` `` and `local a, b, c = 7, pair()` reports
`cannot assign nil to non-nullable type integer`. Both run correctly under
the tree-walker. That is a **third** instance of the same shape as the
interface gap below — the typechecker not modelling something the evaluator
does — and it is now the only thing standing between the compiler and this
construct.

#### `saule-typeck` could not see through an interface method call

`local a: integer = s.half()` on an interface-typed `s` was `cannot determine
the type of this expression`, for a **single-valued** method on an ordinary
program. Root cause: `InterfaceRegistry` is `HashMap<String, Vec<String>>` —
name to `extends` list. It never held signatures, so there was nothing to
look up.

Fixed with a **sibling** registry, `InterfaceMethodRegistry`, rather than by
widening the existing one: six LSP call sites destructure that `Vec<String>`,
and none of them had to change. `lookup_interface_method` walks `extends`
breadth-first — an interface composes by extension rather than inheritance,
so the walk is over a list per level, and a method reached by two paths is
one method.

Threaded through `ModuleSeed` as well, so an **imported** interface's methods
are as knowable as a local one's. Without that the fix would have worked only
in single-file programs, which is not where interfaces are used.

*This is what makes `CALLIF`'s packed result count a live encoding.* It was
added with multi-return and could only be reached through `return`, because
no valid program could bind an interface call to two names. Now one can.

#### A forward reference reached through a callee

The last real divergence, and the one that needed more than a positional
check:

```
class C
    static fn go() -> integer  return later(1)  end
end
local r: integer = C.go()          -- tree-walker: error. VM: 101.
fn later(x: integer) -> integer  return x + 100  end
```

The reference to `later` inside `go` is legal — a forward reference in a
function body is ordinary Saule — and only the **call** is early. Nothing at
the call site distinguishes a callee that reaches an undeclared name from one
that does not.

A blunter guard was tried in an earlier session and reverted: "refuse any
module-body call while a `fn` is still ahead" refused two perfectly good
differential fixtures, because a call partway down a file with any `fn` below
it is an ordinary shape.

The precise version is reachability. A pre-pass records, for each top-level
declaration, the set of top-level names its body **mentions** — one edge per
mention, collected with `saule_ast::visit_stmts`. `Compiler::reaches_undeclared`
closes that transitively at the call site and refuses when it reaches a
declaration the module body has not run yet. Inside a function body it is
vacuously false: by the time one runs, every top-level name exists.

Deliberately one-sided. It over-approximates — a local shadowing a top-level
name still counts as reaching it, and a name mentioned on a branch that never
executes counts too — because over-approximating costs a fallback while
under-approximating costs a wrong answer on a program the tree-walker rejects
outright. **It refuses nothing in either corpus**: 84 of 92 fixtures and 10
of 61 example files compile exactly as before.

The canary `a_forward_reference_reached_through_a_callee_still_diverges` is
repointed and renamed; it now asserts the refusal, plus the two fixtures the
blunt guard broke as the other half of what it is worth.

### `Assignable<T>` — done

`local x: Text = "hello"` builds a `Text` at the **binding site**. The
tree-walker does this in `eval/coerce.rs::to_declared`; a class declaring
`implements Assignable<T>` used to refuse the whole module at layout time.

**This is the one place "reuse rather than reimplement" could not be
followed.** `to_declared` needs an `Environment` to resolve the class name,
and the VM has a class *table* instead. So its decisions are split: the ones
that can be made at compile time are (not a `Named` class, no `Assignable`,
no `of` static — all emit **nothing at all**), and only the two that cannot
are branches: `nil` fills a nullable slot on its own terms, and a value that
is already an instance is returned untouched. `ClassProto` gained an
`assignable` flag, on the **program-global** table because the coercion fires
at a binding site that is usually in a module that only imported the class.

The site list is `coerce.rs`'s, and no wider: an annotated `local`, a module
variable, and a function or method parameter. **Parameter coercion is emitted
after the default entry stubs**, which is the whole of why it works for every
arity — `entries[n_params]` is recorded at the end of `param_entries`, so a
full-arity call enters exactly there and the lower-arity stubs fall through
into it. A copy at pc 0 would be jumped straight over by any call landing on
a stub, and the first cut did exactly that.

Five differential tests, including the shadowing guard: a module-level
`local Text = 1` must not make the class's `of` fire.

Closes `tests/assignable.sau` and `examples/wrapper-types` — the **last**
example project that was falling back for a reason other than design.

### Prelude enums in `match` — done

`case FsKind.File` refused as `a variant of an unknown enum`, which is what
sent `examples/fs-info-example` to the tree-walker. `FsKind` and `OsPlatform`
are defined in Rust and are in no module's layout table — but
`install_*_enum` numbers their variants by declaration order, so their tags
are dense and fixed before a program runs, exactly like a compiled enum's.
`variant_tag` now falls back to the prelude, guarded by `not_shadowed`
(trap 1). A name the enum does not declare now says so, rather than claiming
the enum is unknown.

### Compound assignment — a silent miscompile, found by lifting a refusal

Trap 3, and the sharpest instance of it this project has produced.

`compound_assign` builds a synthetic `target op value` node holding a
**clone** of the target and then assigns to the target again, so every
sub-expression of the target ran **twice**. `t[idx()] += 1` called `idx`
twice under the VM and once under the tree-walker: wrong value, exit status
0, and **present at `HEAD` (`5c9325f`) before any of this work** — verified
against the pre-existing binary, not inferred.

Nothing could see it. The only fixture that writes that shape,
`tests/compound_assign.sau`, also compound-assigns to a *member* two lines
later; that refused, so the whole file fell back and the index bug never ran.
Lifting the member refusal is what exposed it — a refusal standing next to a
miscompile, which is exactly what trap 3 warns about.

The rule now: a compound assignment compiles only when re-reading its target
is **unobservable**. `self`, a bare name and a literal qualify; a call, a
nested index or a chain does not, and refuses so the module falls back to the
engine that evaluates it once. That closes the miscompile and opens
`self.n += 1` and `Counter.total += 1`, which are the shapes real code
writes.

`tests/compound_assign.sau` still falls back — on its deliberate
`t[idx()] += 1` line — and that is now the *correct* outcome rather than a
gap. Lifting it properly means rebuilding compound assignment to resolve its
target into registers once, which needs a register-level binary emitter
(`binary_to` works on AST nodes, because it needs `num_of_node` and
`class_of_expr` for typed opcodes and operator overloads). Its own task.

The canary `unsupported_constructs_report_rather_than_miscompile` now stands
on this refusal — deliberately, because unlike its four predecessors
(`import`, a pipe, a tuple pattern, a compound assignment to a member) it is
*principled* rather than unfinished, and should not need moving again.

### A default skipped in the middle — done, by restriction

`Ui.panel(title: "inner") do … end` skips the defaulted `pad` while supplying
the `body` that follows it. Entry stubs fill a **suffix**, so no entry point
means "fill slot 1 but not slot 2", and this refused.

**A scalar literal default is materialized at the call site.** That is sound
for exactly the reason §19 says a general default is not: a literal reads
nothing from the callee's frame and nothing from the callee's module scope,
and has no side effect to happen in the wrong place or at the wrong time — so
evaluating it at the call site is observationally identical. The same
argument, and the same restriction, is why a valued enum variant's value must
be a literal. `Int`, `Float`, `Bool`, `Str`, `Nil` and a negated numeric
literal qualify.

The node is **rebuilt from the call site's span** rather than cloned from the
declaration: the declaration's `NodeId` belongs to the callee's module and
would answer the wrong module's binding and type tables for an imported
callee.

Anything else — a call, a name, a table literal — still refuses, now as
`a skipped parameter whose non-literal default must run in the callee`. The
restriction is not merely conservative, and a differential test pins why:
`fn f(a, d = a * 2, t)` called as `f(a: 3, t: "!")` must yield `6` from the
callee's `a`, not `200` from a caller that happens to have its own `a`.

Closes `tests/trailing_block_layout.sau` and `examples/ui-blocks`.

### Tuple patterns and nested payload patterns — done, and the bug it hid

Two gaps in one walk. `case (q, r)` refused outright, and a variant payload
had to be all plain binds (`a nested pattern in a variant payload`).

**A tuple pattern needs the scrutinee's whole value list**, which is what the
oracle's `eval_values` produces. Only a **top-level** tuple pattern does: the
oracle recurses with `from_ref(val)`, so a tuple nested inside a payload is
matched against a single value. A `match` with no top-level tuple pattern
therefore still evaluates its scrutinee into one register and emits exactly
the code it did before.

**`NVALS` is a new opcode, and the reason it is needed is worth stating.**
The oracle fails a tuple pattern when `values.len() < elems.len()`, and that
is reachable in a well-typed program — the typechecker accepts
`case (a, b, c)` over a two-value call, which simply does not match. A
compiler that evaluated the scrutinee into a fixed window and padded with nil
could not tell "returned nil" from "returned nothing", so it would match an
arm the tree-walker rejects. `NVALS A B` writes `top - (base + B)`: the count
the callee actually set. Appended after `TAILCALLS`; `opcode_numbering_is_stable`
now pins `NVALS` as last.

`arm_test` and `bind_payload` collapsed into one recursive `test_and_bind`,
which is what closes the nested-payload half for free.

**Two bugs this uncovered, one silent and one loud.**

1. **`NVALS` wrote over `values[1]`.** With `Want::All` a call writes as many
   results as the callee returned, which can be *more* than the window the
   register allocator sized for its **arguments** — `store_results` grows the
   stack and the allocator never hears about it. So a register allocated
   after the call aliased the second result: `case (q, r)` on `return 4, 0`
   bound `r` to **2**, the count. Wrong value, exit status 0, and
   `SAULE_DIFF=1` could not have seen it because no fixture paired a
   multi-return scrutinee with a literal element. Every register the match
   needs is now reserved **before** the scrutinee is evaluated, which makes
   the two ranges disjoint by construction.
2. **A pattern wider than the callee's return read off the end of the frame.**
   `case (a, b, c)` copies three elements out of a two-value window, and the
   third register was past `max_regs`. Allocating above the window raises the
   high-water mark that becomes `max_regs`; the registers are never used.

**And a live divergence this would have created, caught by trap 2.**
`switchable` accepted any variant arm regardless of its payload, which was
safe only because `bind_payload` refused every non-`Bind` field — an inert
gap holding up a fast path. Widening it would have sent `case Shape.Circle(0)`
down the jump-table path, where the arm is entered by a table dispatch and a
failing payload sub-pattern **has no next arm to jump to**. `switchable` now
requires every payload sub-pattern to be irrefutable, and `match_switch`
`debug_assert!`s that no failure label reached it. A differential test asserts
on the disassembly that an irrefutable payload still reaches `SWITCH`, so the
guard cannot quietly turn the fast path off.

Closes `tests/match_tuple.sau`. Six differential tests; the canary
`unsupported_constructs_report_rather_than_miscompile` moved on to
`a compound assignment to a member`, its fourth construct.

### Named arguments across a module boundary — done

The top *real* cause, once the per-project census replaced the per-file one.
`ui-blocks` and `todo-app` both fell back on their first `Panel(title: "…")`
with `a named argument to a callee the compiler cannot identify`.

**The class resolved; its parameter list did not.** `layouts` has been
program-global since the imports slice, so `Panel` was found. But `Tables`
carried only classes, interfaces, enums and the slot counter across modules —
`Compiler::callee_params`, which §19 needs to turn `title:` into a position,
was rebuilt from scratch for every module by Pass 1b's walk over
`module.stmts`. A class declared elsewhere therefore had no entry, and
`reorder_args` had nothing to reorder against.

`Tables` now carries two more maps, and **the split between them is the whole
design**:

- `method_params`, keyed by `(ClassIdx, name)`. A `ClassIdx` is
  program-global, so an entry written by the declaring module answers
  correctly for every importer. Safe to accumulate wholesale.
- `fn_params_by_slot`, keyed by a top-level `fn`'s **program-global module
  slot**. A `CalleeKey::Function` is a bare *name*, and two modules may each
  declare `fn tag`; accumulating those by name would let one answer for the
  other and **swap the arguments silently** — a wrong answer, not a fallback,
  and exactly trap 1's shape. So the publisher keys by slot, and each module
  seeds only the names it actually imports, by walking its own
  `ImportBinding`s (`local` → `from`). Aliases fall out for free: the
  exporter's list is bound under the importer's name for it.

Post-order is what makes both sound — every module is compiled after the ones
it imports, so an entry is always present before an importer needs it.

The seed runs *before* the module's own Pass 1b declarations, so a local `fn`
overwrites an imported one of the same name, matching the resolver's
shadowing order.

**A second gap surfaced while testing the first.** The leak test's initial
form called an imported `fn` with named arguments and refused — class methods
had been made reachable across the boundary, plain exported `fn`s had not.
That is what `fn_params_by_slot` exists for; without writing the guard test
it would have shipped as a still-live half of the same gap.

Four tests in `program.rs`: the imported constructor in both argument orders
(reversed order is what proves the reorder ran rather than the names merely
being dropped), an imported `fn` under its own name and under an alias, and
the no-leak guard — two modules each declaring `fn tag` with the parameters
in opposite order, where a name-keyed map would print the other module's
answer.

| | Before | After |
|---|---|---|
| `examples/*` projects running fully on the VM | 5 of 11 | **6 of 11** |
| projects falling back in `run_examples_diff.sh` | 4 | **3** |

`todo-app` closed outright. `ui-blocks` advanced to the next refusal —
`a skipped parameter whose default must run in the callee`, the one sub-item
still open under item 11 — which is the honest way to read a
first-refusal-wins census: closing one cause reveals the next.

### Tail calls — done, and the two bugs it uncovered

`return f(args)` **replaces** the running frame instead of nesting inside it,
in both engines. The tree-walker got a trampoline first (commit `3a9b6f7`);
this is the VM half, and it was not a feature but a **live divergence**:

```
fn countdown(n: integer, acc: integer) -> integer
    if n == 0 then return acc end
    return countdown(n - 1, acc + n)
end
println(countdown(100000, 0))
```

`5000050000` exit 0 under the tree-walker, `stack overflow: evaluation
nested more than 10000 levels deep` exit 1 under `--vm`. No fixture had the
shape, so none of the three `run_tests.sh` modes could see it.

**The rule is the tree-walker's**, in `Stmt::Return`: a single returned
expression that is a call, whose callee is **not** a `Member`/`SafeMember`,
and which evaluates to a `Value::Function`. Two things then veto it, and
both are properties of the enclosing function rather than of the call:

* **Inside a `try` body.** `exec_try` forces the call to happen for real,
  because the handler must still be on the stack when the callee runs.
* **The module body.** `run_in` does the same — a module body is not a
  function, so there is no frame to replace.

The compiler settles both once, in `Compiler::ret`, and passes `Want::Tail`
only when neither applies. It is a *request*, not a promise: only the shapes
that can genuinely replace a frame honour it, and everything else reports
`tail: false` and gets its ordinary `RET`.

#### Three opcodes, because two of the three call forms have no callee register

| Opcode | Covers | Callee |
|---|---|---|
| `TAILCALL` | a value — a local holding a lambda, an upvalue, a module slot | `R[A]`, decided at **run time** |
| `TAILCALLK` | a top-level `fn` | module/proto packed 8/16 in `EXTRAARG` |
| `TAILCALLS` | a bare-name `static fn` | declaring class/slot packed 8/16 in `EXTRAARG` |

`TAILCALL` alone would have been enough only if a static method's value
could be loaded into a register, and no opcode does that — `GETSTAT` reads a
static *field*. `TAILCALLS` earns its place regardless: `class Main` with a
`static fn` is the idiomatic shape of a Saule program, so it is where the
commonest tail-recursive function in the language actually lives.

`TAILCALL` dispatches like `CALL` because *whether* it is a tail call is a
run-time question there — a local can hold a lambda or a native. A native, a
constructor, anything else callable has no Saule frame to replace, so it is
an ordinary call made on the spot and returned, which is word for word what
`Stmt::Return` does.

**`ret_to` and `n_ret` are inherited by the replacing frame**, so multi-return
survives a tail chain with nothing added: `two() -> one() -> pair()` still
delivers two values to a parallel `local`. Upvalues are closed at `base`
before the arguments move down — a tail call ends a frame just as surely as
a return does, and skipping that would hand a closure whatever the next
iteration wrote.

`TAILCALLK`/`TAILCALLS` carry an `EXTRAARG`, so the *physically* last word of
such a proto is the `EXTRAARG` rather than the tail call. `verify.rs`'s
"runs off the end" check now steps back over it.

#### Bug 1: the fixture that stopped being a test and became a hang

`tests/ui/stack_overflow_recursion.sau` was `return forever(n + 1)` — a
**tail** call. Once the tree-walker trampolined it, unbounded recursion
became an unbounded *loop*: both engines spin, and `run_tests.sh` has no
per-fixture timeout, so the whole suite hangs rather than fails. Pre-existing
at `HEAD`, and invisible unless you actually run the suite to completion.

Fixed by binding the result first, so the call is no longer in tail position
and the fixture tests what its comment says it tests. The comment now says
not to "simplify" it back.

#### Bug 2: `try return f() catch` stopped catching

Found by the VM, which was *more* correct than the oracle — the first time
that has happened on this project.

`exec_try` forces a tail call into a real one so the handler stays live, and
that is right. But it made the call with a `?`:

```rust
Ok(Flow::TailCall { callee, args, span }) => Ok(Flow::Return(
    call_function_multi(&callee, &args, span)?,   // <- escapes this `try`
)),
```

The `?` propagates past `exec_try`'s own `Err(Thrown)` arm, so the callee ran
outside the very handler that forced it:

```
fn boom() -> integer  throw "bang"  end
fn guarded() -> integer
    try return boom() catch e: any return -7 end
end
```

`uncaught exception: bang` under the tree-walker, `-7` under the VM. The fix
folds the forced call back into the same `Result` the body produced, so one
`match` sees both. Pinned by
`a_try_around_a_tail_call_still_catches_what_the_callee_throws`.

#### Measured: tail calls buy depth, not speed

Neither engine got faster. Release, min of 5–7, same binary with only the
tail-call path differing:

| | tree-walker | VM |
|---|---|---|
| Best benchmark | `strings` +0.7% | `map` +4.2% |
| Worst benchmark | `oop` −2.4% | `fib` −2.3% |
| Mean | ≈ −0.5% | ≈ −0.1% |

Every figure is inside the ~3% noise floor. That is expected rather than
disappointing: no benchmark contains a qualifying tail call. `fib` returns
`fib(n-1) + fib(n-2)`, which is a binary operation and not a call, and the
rest wrap their work in `class Main` / `static fn main()` where the inner
calls are methods. What changed is that `countdown(100000, 0)` returns an
answer instead of overflowing.

#### Negative cases are asserted on the *disassembly*, not on depth

"This must **not** be a tail call" could be shown by recursing past the depth
guard, but 10 000 tree-walker frames overflow libtest's thread even at
`RUST_MIN_STACK=16777216` in a debug build — which is how
`a_method_call_in_tail_position_is_not_a_tail_call` first failed. Reading the
opcode back out of `disasm_of` is exact, cheap, and fails for the right
reason. The positive cases still recurse 50 000 deep, because constant depth
is precisely what they are asserting.

### Multi-return and parallel binding — done, and the divergence it exposed

`local a, b = f()`, `a, b = b, a`, and `return f()` passing a callee's
results **through**. Four fixtures (`fibonacci`, `string_lib`,
`call_return_inference`, `multiple_return_values`), and the last item on
§21.4's list of language features.

**The rule is `eval_expr_list`'s, and it is narrower than it looks.** Only
the **last** expression of a value list expands, and only when it is a call
— `eval_values` matches `Expr::Call` and returns a one-element list for
everything else. Extra names are nil; surplus values are dropped *after*
being evaluated. Reproducing that exactly is what `compile/stmt.rs`'s
`expr_list_to` does, and every clause of it has a differential test.

**A count the compiler knows needs no `top` at all.** A parallel `local`
knows how many registers it is filling, so `C = nret + 1` asks for exactly
that and `pop_frame` pads a short callee with nil and drops a long one's
surplus — the tree-walker's rule, for free, the same way the `for … in`
driver already got it. `top` is needed in exactly one place: `return f()`,
where the count is a run-time fact. There the call takes `C = 0` and the
return takes `B = 0`.

**`B = 0` on a *call* remains unimplemented on purpose.** `f(g())` passes
one argument in Saule: `eval_call_args` evaluates each argument with `eval`,
not `eval_values`. Implementing argument expansion would be inventing a
language rule rather than matching one.

#### The bug this found before a line was written

`return f()` compiled to `RET1` and **truncated the callee's results to
one**. Under the tree-walker it returns all of them. That was live, silent,
and reachable today — not by a parallel `local`, which did not compile, but
through a `for … in` **driver**, which asks for `nvars` results:

```
fn pair() -> (integer, integer) return 11, 22 end
fn wrap() -> (integer, integer) return pair() end
-- a driver whose body is `return wrap()`
for a, b in mkdriver() do println(a, b) end
```

`11  22` under the tree-walker, `11  nil` under the VM. **Exit status 0, no
error, wrong value.** No fixture pairs a pass-through return with a
multi-value consumer, so `SAULE_DIFF=1` could not see it either. Pinned now
by `a_returned_call_through_a_driver_yields_every_value`.

Worth drawing out, because it is the third time this pattern has appeared:
the truncation had been correct-by-accident for as long as nothing compiled
that could consume two values. It became a wrong answer the moment coverage
widened — the same shape as `EnumObject.methods` and the `toString` guard.
**Re-read what a gap is "known safe because of" whenever you close a
neighbouring one.**

#### Two opcodes had no room for a result count

Both spend `C` on something else, which is exactly the pressure §15.10
anticipated:

- **`CALLM`** carries the vtable slot in `C`. `CALLM_MR` — reserved in the
  opcode table since Phase 1 and unimplemented until now — moves the slot
  into `EXTRAARG` so `C` can be `nret + 1`. A call wanting exactly one
  result still takes the cheaper `CALLM`, so nothing on the hot path
  changed.
- **`CALLIF`** carries the interface's *method* slot in `C` and the
  interface index in `EXTRAARG`. The count now rides packed 8/16 in that
  `EXTRAARG`, the same split `CALLK` uses for its module. This is a **chunk
  ABI change** to an existing opcode's operand, which is worth noting: the
  numbering is untouched, but a cached chunk from before this change would
  decode the count as 0.

  It was briefly implemented as a *refusal* instead, on the argument that no
  program could reach it — and that was wrong. `return s.area()` on an
  interface-typed receiver asks for all results, which is ordinary code;
  refusing it made `one_call_site_dispatches_to_two_implementations` fall
  back. A **parallel `local`** from an interface call genuinely cannot be
  written yet, but for an unrelated front-end reason (below).

#### Refused rather than guessed

- **`return a, f()`** — several values whose last is a call. The returned
  range has to be contiguous, so `f`'s window would have to begin exactly
  where `a` ends, which the bump allocator cannot promise while `a` is
  still live. Refusing costs a fallback; truncating would cost a wrong
  answer.
- **`return x?.m()`** — a safe method call's nil arm has to produce as many
  results as the call arm, and for an unknown count that means setting
  `top`, which no opcode does. `local a, b = x?.m()` is fine, because there
  the count is known and the nil arm is just `n` × `LOADNIL`.

#### Noticed, not fixed: typeck cannot see through an interface method call

`saule-typeck` reports `cannot determine the type of this expression` for
**any** call on an interface-typed receiver, single-valued ones included:

```
fn use(s: Splitter) -> string
    local a: integer = s.half()   -- cannot determine the type
end
```

So `return s.area()` works only because a `return` position does not demand
one. This is a front-end gap with nothing to do with the VM, and it is why
the `CALLIF` multi-result encoding is currently exercised through `return`
and not through a parallel `local`.

### Open divergence: a module-level forward call

**Found while adding pipes; it has nothing to do with pipes.** At module
level, calling a top-level `fn` declared *below* the call site:

```
local r: integer = later(5)      -- tree-walker: error. VM: 105.
fn later(n: integer) -> integer
    return n + 100
end
```

The tree-walker executes top-level statements in order, so `later` is not
bound yet and it reports `identifier 'later' reached evaluation undefined`.
The compiler reserves proto indices in a pre-pass — which is *correct* and
necessary inside function bodies, where `fn a() b() end` above `fn b()` is
ordinary Saule — and applies the same resolution to the module body, so
`CALLK` happily calls it.

The VM is the more permissive engine here, which is the wrong direction: the
tree-walker is the oracle. `when(5):later()` inherits it identically, which
is why the pipe tests do not cover that shape.

**Not fixed, and it is not a one-liner.** The fix is to let `CALLK` resolve a
top-level `fn` from the *module body* only once that `fn`'s declaration has
been emitted, while leaving forward references inside function bodies alone
— so the compiler needs to track which top-level `fn`s the module body has
passed. No fixture has this shape, so `SAULE_DIFF=1` does not see it.

### Two bugs in `match` guards — one silent

Chasing "a local the compiler has not seen declared" found the refusal that
was reported *and* a miscompile sitting next to it. Both came from one
ordering mistake in `compile/match_.rs`: the guard was emitted inside
`arm_test`, before the arm's scope was entered.

1. **The refusal** (`case x when x < 0`). The pattern's binding was not in a
   register yet when the guard compiled, so the guard's `x` looked like a
   local the compiler had never seen. Safe — it fell back — but it also
   refused `case Event.Click(x, y) when x > 0`, the same rule for a
   destructured payload.

2. **The miscompile.** With a guard present, `arm_test` patched the
   *pattern's* failure jump to just past the guard's jump — which is where
   the arm **body** starts. So a pattern that did not match ran the arm
   anyway:

   ```
   local n: integer = 5
   match n
     case 0 when true then "zero"     -- VM: taken. Tree-walker: not.
     case _ then "other"
   end
   ```

   `zero` under the VM, `other` under the tree-walker. **Exit status 0, no
   error, wrong value** — and `SAULE_DIFF=1` could not see it, because no
   fixture pairs a *literal* pattern with a guard. `tests/match_guard.sau`
   uses only binding patterns, which always match, so the mis-patched jump
   was never taken there.

The fix orders the three steps explicitly — test the pattern, bind what it
introduces, then test the guard — with both failure jumps patched to the
next arm by the caller rather than chained inside `arm_test`. Pinned by
three differential tests, including the literal-pattern-with-guard shape
that nothing covered.

*Worth drawing out:* the reported symptom was a clean refusal, and the
unreported one next to it was a wrong answer. A refusal is a signal that the
compiler mishandles a construct — not a reason to assume the mishandling is
confined to the part that refused.

### The tree-walker can now call into bytecode — done

One root cause with four symptoms. `Value::VmFunction` was opaque to
`saule-interpreter`, and a bytecode method is a `Proto`, not a
`FunctionObject`, so the runtime `ClassObject` the VM built had an **empty
method map**. Every path where the tree-walker's own code needed to call a
user function on a value hit that wall.

**What was built**, in the recommended shape:

- [x] `Vm` split into an `Rc<VmShared>` — chunk, module slots, statics,
      enums, classes, closure cache — plus per-invocation state (stack,
      frames, open upvalues). A callback runs a **fresh** `Vm` over the same
      shared half, which is what dissolves the `&mut self` the dispatch loop
      holds across a `CALLNAT`.
- [x] `VmFunction` gained a `call` method; `call_value_multi` gained a
      `Value::VmFunction` arm. `call` takes the erased `Rc<VmFunctionRef>`
      **handle** alongside `&self`, which looks redundant and is not: a VM
      frame holds an `Rc<VmFunctionRef>` and `&self` cannot be turned back
      into one, so without it every callback would rebuild an equivalent
      closure — an allocation per comparison on a sort.
- [x] Class method maps hold `MethodRef { Tree(Rc<FunctionObject>),
      Vm(Rc<VmFunctionRef>) }` rather than `Rc<FunctionObject>`. Built from
      the chunk's `vindex`/`vtable`, both of which are prefix-extensions of
      the parent's, so an inherited method is one probe away exactly as it
      is on a tree-walker class.

**`Closure` holds a `Weak<VmShared>`, not an `Rc`.** A closure very commonly
lives in a module slot and a module slot lives in `VmShared`; a strong
pointer would close that cycle and leak the whole program's state — the same
shape as the capture leak `closure_semantics.rs` exists to guard against.
The running `Vm` holds the strong reference, so the upgrade succeeds exactly
when calling makes sense. `Rc::new_cyclic` is what lets the classes built at
start-up carry method closures that already know their own engine.

**Recursion is bounded by `saule_interpreter::enter_call_depth`, not by
`max_frames`.** This is the one place the recommended shape was wrong.
`max_frames` counts frames within *one* `Vm`, and each re-entrant call is a
fresh `Vm` with frames of its own — so it cannot see the nesting at all.
What nesting consumes is the **native** stack, one Rust frame per level,
which is what the tree-walker's own depth guard already counts. Sharing the
counter also bounds a program bouncing between engines once rather than
twice. Pinned by `tests/ui/stack_overflow_reentrant.sau`.

**The two guards this deleted:** `refuse_closure_to_native` in
`compile/expr.rs`, and the `toString` refusal in `compile/layout.rs`.
`sort.sau` compiles, taking the benchmarks to **10 of 10**.

#### The bug the `toString` guard was hiding

Lifting the refusal turned `SAULE_DIFF=1` red immediately, on
`operator_overload.sau`: `"cost: " .. money` printed `300c` under the
tree-walker and `<instance of Money>` under the VM. `Op::CONCAT` and
`Op::TOSTR` were reading `Value::to_display_string()` directly instead of
going through `ops::display_value`, so they never consulted the overload —
**reuse rather than reimplement, broken in the one place it was easiest to
miss**, and invisible to exit status. Both now call `display_value`.

`CONCAT` also had to stop rendering each operand twice. It used to make one
pass to measure the result's length and a second to build it, which is
harmless while rendering is pure and wrong the moment an operand's
`toString` is user code with side effects. It now renders once into a small
`Vec<String>` — which is also *fewer* allocations than before, since
`to_display_string` allocated on each of the two passes.

**`EnumObject.methods` is still empty**, deliberately. The same trap in a
different place, so it is worth saying why it is not one: the compiler
refuses an enum method call outright (§0.6 — an enum method is a bare
`Method` with no `Spanned`, so it has no `NodeId` to key a `FunctionInfo`
on), so the module falls back before a VM enum ever reaches a method lookup.
And the two dynamic paths that read a class's map cannot reach an enum at
all — `has_overload` matches only `Value::Instance`. If enum methods are
ever compiled, this map has to be filled in the same sweep.

#### Still open: a method read as a value

Symptom 4 is the one this did **not** close, and it should not be closed
without deciding what `obj.method` means. The runtime half is in place —
`read_member` hands back `MethodRef::to_value()`, so a bytecode method now
yields a callable `Value::VmFunction` instead of nothing — but:

* the compiler still refuses the member read itself (`a member that is
  neither an instance field nor a static`), and
* the **tree-walker's own** behaviour here is broken, and was before this
  work: `read_member` returns the method unbound, so calling it fails with
  `internal: `self` reached evaluation outside a method`. Verified against
  `df8a1eb`.

So there is no oracle to be differential-tested against yet. Fixing the
tree-walker's semantics is the prerequisite, and it is a language decision
(bound method value? explicit receiver?) rather than a VM one.

### `Assignable<T>` is refused

`local x: Text = "hello"` builds a `Text` **at the binding site**, from the
declared type (`eval/coerce.rs`). Nothing in this compiler does that, so a
chunk would store the bare string and the first method call on it would
find a `string` where an instance was promised — which is what
`tests/assignable.sau` did, once nullability let the rest of the file
compile. A class declaring `implements Assignable<...>` now refuses the
module at layout time. Implementing it means coercing at annotated
`local`s, module variables, and user-function parameters and returns —
exactly the closed site list `coerce.rs` documents, and no wider.

### Nine fixtures the VM ran wrongly — all fixed

Pre-existing, and **not** caused by the nullability slice — verified by
gating every new codegen path off and re-running: the same nine failed
either way. Every one was the same shape: **the compiler emitted code for
something it did not actually support, instead of refusing.**

`SAULE_ENGINE=vm` is back to **235/235** (236 as the suite stands today).

| Was failing | Cause | Fix |
|---|---|---|
| `shapes`, `fn_type_variance` | **Inherited vtable slots were never filled.** Pass 1 copies the parent's vtable so the slot *numbering* extends it — but no body is compiled yet, so what it copies is a row of `u32::MAX`, and `class_decl` fills only the slots a class declares itself. `Circle.describe`, inherited and not overridden, stayed unfilled. | Pass 2a: one forward sweep after codegen, parents before children (which `order_by_depth` already guarantees), filling any slot still `u32::MAX` from the parent. |
| `operator_overload` | `compare` and `equals` return a *value*, not the operator's answer. The overload path used the raw result, so `b < a` evaluated to `-180`. Unary `-` had no compile-time path at all. | Post-process the way `ops::binary` does — `compare` read against zero for all four orderings, `equals` normalised through `NOT`/`NOT` and negated for `!=`. Unary overloads resolved at compile time like the binary ones. |
| `op_index` | `GETIDX`/`SETIDX` are table-only despite §15.9 calling them the dynamic form, so an instance receiver hit "expected `table`". | `OpIndex`/`OpNewIndex` resolved to vtable slots at compile time, same pattern. |
| `iter_closure`, `iter_object`, `iter_pairs`, `ui/iter_missing_iter_method` | `ITERPREP` emitted for a `for … in` over a closure or an instance — the closure-driver path (item 5) is not written. | Refuse unless the source is a **proved table**. An unproved table is refused too; that costs a needless fallback, which is the right side to err on. |
| `ui/implements_missing_method` | A class missing an interface method compiled with a hole in its itable. Nothing before the compiler rejects it — the *tree-walker* catches it at class declaration. | Pass 1 refuses when a declared interface has an unmatched method; a new pass does the same for the stdlib contracts, looked up by name in a prelude scope exactly as the tree-walker looks them up. |

Not fixed at the time, refused instead — the closure-driver `for … in` (item
5) and right-operand operator dispatch (item 6). What changed *then* is that
they fell back rather than computing a wrong answer.

**Both are settled now.** The closure driver landed with `ITERPREPX` (item 5)
and compiles. Right-operand dispatch never needed its own path: an operator
the compiler cannot resolve against the left operand's proved class emits
`ARITHX`, which calls `saule_interpreter::eval::ops::binary` — the oracle's
own operator logic, including its right-operand rules — so the answer is
identical by construction rather than by care. Slower than a resolved
overload, never wrong.

### The last four gaps — closed

Four fixtures, four unrelated premises, none of which was the feature it
looked like. Worth reading together: in three of the four the *refusal
message* named a language construct while the actual cause was a missing
`match` arm or a test asked of the wrong frame.

**`export name: T = value` — `Decl::Variable` had no branch.** `decl()`
handled `Function`, waved through `Class`/`Enum`/`Interface`, decided about
`Import`, and refused everything else as `a declaration the compiler does not
handle`. `Decl::Variable` fell into that `else`. There was nothing to build:
`collect_module_scope` already pushes it alongside `Stmt::Local`, so the slot
existed and the store is the same `SETMOD` a module-top `local` compiles to.

One thing this must **not** do is coerce. The module-top `local` path calls
`coerce_to_declared`, and copying it here would have been the obvious move —
but `exec_decl`'s `Decl::Variable` arm evaluates the initializer and defines
the name, full stop, while the `Stmt::Local` arm directly above it calls
`coerce::to_declared`. So `export x: Str = "…"` builds no `Str` under the
oracle, and coercing would have made the VM build one. **Two arms of the same
`match` in the tree-walker do not have to agree with each other, and the
compiler has to copy each one separately.**

**``self` outside a method` — the test was asked of the wrong frame.**
`Expr::Self_` checked `self.f.in_method`, which is false in a lambda's
`FuncCtx` by construction, so a lambda written in a method body — the
`fn describe() … local f = fn() return self.label end … end` shape —
refused. But `method_proto` declares `self` as an ordinary local at register
0 *under the name `self`*, so the capture walk every other free variable
takes already reaches it: `capture_upvalue("self")` and `GETUPVAL`.

The class-static half of the same shape (`count = count + 1` inside that
lambda) already worked, because the resolver carries the *class name* on
`Binding::ClassStatic` rather than a slot — a decision made specifically so
the answer survives a lambda nested inside a method. That is the same problem
solved properly one layer up, and it is why only `self` was left.

**And it uncovered a second refusal beside it, which needed a new opcode.**
`tests/closure_capture.sau` then failed on `local go = fn(k) … go(k - 1) … end`
with `a captured variable the compiler could not locate`. The name is not a
capturable local of the enclosing frame — `local` declares its register
*after* the initializer compiles — and making it one would have been wrong
rather than merely awkward: the closed upvalue cell would hold the closure
and the closure would hold the cell, an `Rc` cycle per call. That is exactly
the leak `FunctionObject::self_name` exists to avoid on the tree-walker side
(2,468 MB against a 7.5 MB control, in the measurement that motivated it).

So `SELFFUNC` — `R[A] := the closure this frame is running`. The handle is
already on the `Frame`, so the recursive call reads it from there and no cell
exists to close a cycle with. **Appended after `NVALS`, never inserted:** the
numbering is the chunk ABI, and `opcode_numbering_is_stable` is the test that
says so — it failed on this change and was extended rather than edited around.

Only the lambda the `local` directly names gets this: `Compiler::binding_lambda_to`
is `take()`n by `lambda_to`, so a deeper nested lambda mentioning the same
name still refuses rather than silently resolving to the wrong closure.

**`a prelude name outside a call` — folded members, unfolded entities.**
`prelude_member` folds `Math.pi` to a `LOADK` but only for `Int`/`Float`/
`Str`/`Bool`/`EnumVariant`. `Io.stdout` is a file handle, so it fell through
to evaluating the receiver `Io` as an expression — and *that* is what
refused. The fix is one layer down: fold the **entity**, so `Io` is a
constant and `Io.stdout.write(…)` is then an ordinary `GETFX` + `CALLMX`
deferring to the tree-walker's own `read_member`.

**This immediately fired a canary, which is what canaries are for.**
`a_reassigned_stdlib_constant_is_not_folded` asserted the two engines
*disagree* on `Math.pi = 3.0`, documenting that the write did not compile and
that the no-fold guard was therefore untested. Folding `Math` made the write
compile, and the test failed exactly as its own comment predicted. The guard
it was protecting is real and needed: the compiler and the tree-walker each
call `Environment::with_prelude()`, and `install` builds a **fresh
`ClassObject` per environment** — so a folded `Math` is not the object the
tree-walker mutates. The bare-name fold now carries the same
`mutated_receivers` gate the member fold does, and `Math.pi = 3.0` falls back.

**`an enum with methods` — the note in this file named the wrong cause.**
The table above said "§0.6's missing `NodeId`, or a different key", and
following that would have meant changing the AST. The `NodeId` was never the
blocker: `resolve/decls.rs`'s `Decl::Enum` arm calls `enter_function` on every
method body, so every identifier inside one already has a binding, and the
frame layout is the compiler's to compute anyway. What a missing `NodeId`
actually costs is the `FunctionInfo` a *caller* would use for named arguments
and defaults — not the body.

The real blocker was a type: `EnumObject::methods` was
`HashMap<String, Rc<FunctionObject>>`, which a bytecode method cannot inhabit.
That is the identical failure `MethodRef` was introduced for on the class
side, and the fix is to reuse it — `methods` is now
`HashMap<String, MethodRef>`, `exec_enum_decl` wraps in `Tree`, and the VM's
start-up pass builds `Vm` closures over the declaring module's chunk. No
vtable, because an enum cannot be extended: a name probe is the whole of it.

Dispatch needed one correction beside it. `dispatch_member_call_multi`'s
`EnumVariant` arm read the member and then branched on the *shape* of what
came back — `Value::Function` got the receiver, anything else did not. A
compiled method comes back as `Value::VmFunction` and would have been called
with no `self` at all. It now looks the name up in the method map directly
and goes through `call_method_ref_multi`, which is what deciding by shape was
approximating. (`value` and `name` still win over a method of the same name,
because `read_member` answers them first; that ordering is mirrored rather
than reordered.)

### A nullable `catch` type caught everything

Found while auditing item 7's remaining `[ ]`, and it was not a gap:

```
try
  throw 42
catch e: string?
  println("caught")
end
```

printed `caught` under the VM and let the exception escape under the
tree-walker. Silent — exit status 0, output present, just the wrong output.

`Compiler::type_desc` mapped anything that was not a `Named`, `Function` or
`Table` type to `TypeDesc::Any`, under a comment saying "a nullable or
generic `catch` type is not a runtime test the tree-walker performs either".
Half right. `runtime_matches_type` really does answer `true` for
`Type::Tuple(_)`, with its own comment explaining why. But its very next arm
is `Type::Nullable(inner) => matches!(value, Value::Nil) || runtime_matches_type(value, inner)`
— a real test, and `Any` is the opposite of it.

`TypeDesc` grew `Nullable(u32)`, an index into the same descriptor pool
rather than a `Box`, so the pool stays flat and a chunk stays as serializable
as it was for §14's cache. `value_matches` recurses.

**The lesson is about the comment, not the code.** One sentence asserted a
property of two AST variants at once, one of which had it and one of which
did not, and that sentence had been read as documentation ever since. A claim
covering several cases has to be checked case by case; this one cost a live
divergence that no fixture exercised, because no fixture throws a
non-matching value at a nullable `catch`. Pinned now by
`a_nullable_catch_type_does_not_catch_everything`, which asserts both
directions — that it does *not* catch the integer, and that it still catches
a string and a nil.

### `www/` is covered now — by the script that was already there

Phase 3's harness criterion carried "`www/` is not covered yet". It is now,
and it needed no new harness: `www/scripts/check-samples.mjs` already
extracted every complete program the site ships — the playground's example
picker plus the hand-written fenced blocks in the guides — and ran each one
through the real compiler. It just ran them **once**, under whichever engine
was the default, which proves they compile and proves nothing about
agreement.

It now runs each sample under `SAULE_ENGINE=vm` and `SAULE_ENGINE=interp` and
compares output, strips the fallback note before comparing (the note is a
property of the engine, not of the program), counts fallbacks, and honours
`SAULE_BIN` like the two shell harnesses do — which mattered immediately,
because `findCompiler` prefers `target/release` and the release binary here
was stale. **20 samples, both engines, identical output, 0 fallbacks.**

What this does *not* cover is the rest of `www/`: the generated pages come
from `README.md` and `DOCS.md`, where most snippets are illustrative
fragments — a lone method body, a type signature — that were never meant to
compile standalone. `HAND_WRITTEN` is the opt-in list, and growing it is how
that coverage grows.

### Phase 3 exit criteria

**All five are met. Phase 3 is closed.**

- [x] All `tests/*.sau` and all `tests/ui/*.sau` behave identically under
      both engines — `SAULE_DIFF=1 ./run_tests.sh`, **236/236**, output
      compared rather than just exit status, two documented exemptions.
      **Note what this does and does not say:** it holds *including* the
      programs that fall back, so it proves the two engines agree, not that
      the VM compiles everything. Read it together with the coverage table
      at the top.
- [x] All 10 benchmarks run under `--vm` — `sort.sau` was the last, and
      re-entrancy is what unblocked it. Re-checked at the close of Phase 3:
      10 of 10, zero fallbacks.
- [x] The differential harness is green across `examples/` —
      `run_examples_diff.sh`, **9 of 11 projects compared, both engines
      agreeing on every one, 0 fallbacks**.

      Different from `run_tests.sh` in the way that matters: those fixtures
      are small, single-file and side-effect-free, while these are real
      multi-module programs with imports and file IO. Every silent
      divergence this project has found came from code shaped like a
      program rather than like a fixture.

      Two projects are skipped, both with the same reason — `UI Project` and
      `toying` open a window and loop until it is closed, so neither has a
      terminating run to compare. They hang identically under *both*
      engines, which is a property of the program, not a divergence. The
      skip count is printed on every run so the list cannot grow unnoticed.

      Projects that write files are snapshotted and restored between the two
      runs, so the second engine starts from the state the first one saw —
      otherwise `todo-app` reports a different second run for a reason that
      has nothing to do with the engine.

      **`www/` is covered too**, which is what this box was waiting on:
      `www/scripts/check-samples.mjs` now runs its 20 complete programs
      under both engines and compares output. See "`www/` is covered now"
      above for what it does and does not reach.
- [x] Coverage: **91 of 92** `tests/*.sau` compile fully, and **9 of 11**
      example projects run entirely on the VM with `run_examples_diff.sh`
      reporting **0 fallbacks**, as do all **10** benchmarks and all **20**
      `www/` samples.

      **Every remaining refusal is a deliberate one.** The 2 projects are
      refused by design (`an import of a dynamic native package` is a
      runtime side effect that compiling must not perform), and the 1
      fixture is `tests/compound_assign.sau`, where the refusal is what
      *fixes* a miscompile. The three real gaps this box used to name — an
      enum with methods, a prelude name outside a call, and `self` outside a
      method — are closed, along with a fourth (`a declaration the compiler
      does not handle`) that this box never listed.

      **What "coverage" still does not mean.** A fixture that compiles fully
      is not a fixture that exercises much: `tests/*.sau` are single files,
      and the per-file `disasm` census over `examples/` (12 of 61) measures
      the single-module path rather than how real programs compile. The
      project row is the honest one, and it is the one to steer by.

**Deferred out of Phase 3, on purpose:**

- The `CALLM` inline cache (§8.5) and the interface-call inline cache (§8.4)
  are **Phase 5**. Both are performance, not coverage, and both want a
  benchmark rather than a guess.
- A valued variant's value must be a **literal**. A chunk stores constants,
  not code; a non-literal is refused rather than mis-evaluated. Not a gap —
  a representation limit, and it blocks no fixture, no benchmark, no example
  project and no `www/` sample.
- `saule-typeck` does not check the **arguments** of a safe method call
  (`g?.twice("no")` passes today). Noticed during item 7, still true, and
  still not a VM item: it adds diagnostics to a working language and belongs
  in its own change.

---

# Phase 4 — Flip the default

*Estimate: 1–2 weeks. **Done**, except the one item that is a release rather
than a change — see the note under it.*

- [x] `--vm` becomes the default; `--interp` selects the tree-walker.
      `run.rs`'s `use_vm() -> bool` became `engine() -> Engine`, because the
      question stopped being yes/no: the VM can now be *defaulted* into or
      *asked* for, and the two differ in one visible way — see the next item.
      `SAULE_ENGINE` gained `interp`, and clap's `conflicts_with` rejects
      `--vm --interp` rather than picking a winner.
- [x] **The fallback note is printed only when the VM was asked for.**
      Not on the original list, and the one place where flipping a default was
      not mechanical. ``note: the bytecode compiler does not handle `X` yet``
      was useful advice while `--vm` was opt-in and is noise once it is the
      default: it fires on 4 of 9 example projects, on every run, about a gap
      the user cannot act on, for a fallback that by construction changes
      nothing observable. `--vm` and `SAULE_ENGINE=vm` restore it — which is
      also what keeps `run_examples_diff.sh`'s fallback count working, since it
      sets `SAULE_ENGINE=vm` explicitly, as does `run_tests.sh`.
- [x] `saule-wasm` switches `run` / `check_and_run` to the VM.
      The single-module route (`saule_vm::compile` + `run_chunk`) — a browser
      has no filesystem for an import graph to live in — with the CLI's
      fallback discipline. Three things worth keeping:
      **(a)** compiling happens *outside* `output::capture`, or a program that
      printed and then fell back would print its output twice;
      **(b)** `saule-vm` grew a `native-packages` feature that is a pure
      passthrough to `saule-interpreter`'s, and now depends on the interpreter
      with `default-features = false`. Without that, `saule-wasm` pulls
      `libloading` back in *through the VM* and stops building for wasm32 —
      the exact blocker that feature was added to remove. The check that
      catches it is `cargo check -p saule-wasm --target wasm32-unknown-unknown`,
      not `cargo build`, which passes happily on the host;
      **(c)** every test in that crate passes under either engine, because the
      fallback is behaviour-preserving by design — which is exactly what would
      let the wiring rot back to "always the tree-walker" unnoticed. So one
      test asserts the *compile* step succeeds on the playground's own showcase
      program.
      A `Phase::Compile` was added to the playground's JSON contract, for a
      compiler *fault*; an unsupported construct is not one and never reaches a
      diagnostic. `www/` renders `phase` as a string, so the new variant needs
      no front-end change.
- [~] One release ships with both engines and a documented escape hatch.
      The escape hatch is documented: README's new "Execution Engines" section
      covers `--interp`, `--vm` and `SAULE_ENGINE`, and says plainly that
      needing `--interp` is a bug worth reporting with the program that needs
      it. **The release itself has not shipped**, and cannot be ticked from
      inside the tree: `git tag` still shows one tag and the release workflow
      has never published (`PRODUCTION.md` §1 calls that the binding
      constraint on everything else). This box closes when a release does.
- [x] Update `PRODUCTION.md` §"How fast is it?", the grade table, and
      Appendix A with **real measured numbers**. Measured on this box, not
      carried forward: 1.0×–4.8× PUC Lua 5.4.8 against the tree-walker's
      5.5×–9.0× in the same conditions; runtime-performance grade C – B.
      (Re-measured later on macOS arm64 against Lua 5.5.0: 1.1×–4.5× for the
      VM and 1.3×–12.8× for the tree-walker. Different machine and different
      Lua, so it is a second row in Appendix A rather than a correction.)
      §3.3, §3.6 and §10's Phase 6 were rewritten too — each of them argued
      *for* building a VM, and reads wrong once one exists.
      The LuaJIT column was **removed** rather than reused: it was not installed
      here, and the old 30–90× came from a different OS and architecture. A
      stale column beside a fresh one reads as a comparison that was not made.
- [x] `saule-lsp` and `saule-db` need no changes — confirm, don't assume (§14).
      **Confirmed, and this is the confirmation rather than the claim.** Neither
      crate depends on `saule-vm`, and neither executes user code: every
      `saule_interpreter::` reference across both is a static-fact API —
      `init`, `all`, `all_prelude_names`, `export_names`, `lookup`,
      `package_names`, `is_dynamic_package`, `resolve_import_path`,
      `collect_import_seed{,_io}` — with no `run`, `run_in`, `check_and_run` or
      `call_class_static_method` anywhere. There is no engine for them to pick.
- [x] Keep the tree-walker in-tree for at least one full release cycle. It is
      the differential oracle and it is ~13k lines that already work.
      Nothing was deleted. §22.1's one-way dependency arrow is what makes this
      cheap to honour: `saule-vm` depends on `saule-interpreter`, never the
      reverse, so the oracle stays buildable and testable with the VM removed
      from the workspace.

## Verification at the flip

All five commands, plus the two the flip made newly meaningful. Debug build
unless stated.

| Command | Result |
|---|---|
| `cargo test --workspace` | **1405 passed, 0 failed, 5 ignored** |
| `run_tests.sh` (no `SAULE_ENGINE` — now the VM) | 236/236 |
| `SAULE_ENGINE=vm run_tests.sh` | 236/236 |
| `SAULE_ENGINE=interp run_tests.sh` | 236/236 |
| `SAULE_DIFF=1 run_tests.sh` | 236/236, engines agree on output, 2 exempt |
| `run_examples_diff.sh` | 9 of 11 compared, 4 fell back, all agree |
| `--example compare` (release) | agrees, then 2.63×–3.74× |
| `benchmarks/bench.py check` | VM, tree-walker and Lua print identically |
| `cargo check -p saule-wasm --target wasm32-unknown-unknown` | clean |

The bare `run_tests.sh` row is the one that was not a test before: with no
`SAULE_ENGINE` set it used to exercise the tree-walker and now exercises the
VM, which is the flip itself.

## What flipping the default did *not* fix

The compiler's coverage. The flip is safe because the *fallback* is safe, not
because the gap closed — at the flip, 5 of the 9 comparable example projects
ran fully on the VM and 4 took the tree-walker, unchanged by any of this work.
The earlier handoff's rule ("do not start Phase 4 until real-program coverage
is well above 4 of 9") was a rule about *value*, not safety: what it guards
against is shipping a default that is mostly a no-op on real code, and that
risk is real and still open. `run_examples_diff.sh` prints the fallback count
on every run and that is the number to steer by — not "236/236 under
`SAULE_ENGINE=vm`", which scores a fallback as a pass.

**Deliberately not done here: raising the frame limit.** The previous handoff
called it Phase 4 work, and it is not on this checklist. `MAX_EVAL_DEPTH` is
aligned at the tree-walker's 10 000 and §6.4 argues for a million, since a
call under the VM is a `Vec` push rather than a native frame — but that
changes which programs the *language* accepts, not which engine runs them, and
it has to move in both engines in one change or `depth(50_000)` diverges again.
Its own task, with its own differential fixture.

---

# Phase 5 — Optimization

*Ongoing, and **only with a profile in hand**. Started; the first slice is
the emission peepholes below, which the profile asked for ahead of every
candidate this phase was originally written around.*

- [x] **`--profile-bytecode` — the profile the rest of this phase requires.**
      §16 says every superinstruction "must be justified by a profile before
      it is added" and names the collector; `crates/saule-vm/src/profile.rs`
      is it. `saule run --profile-bytecode <target>` prints two tables to
      stderr when the program finishes: executions per opcode, and
      executions per **statically adjacent** opcode pair, each with a share
      and a running cumulative share.

      **Adjacent, not "whatever ran next"**, and the distinction is the
      whole point. A superinstruction is emitted by the compiler, which can
      only fuse two words it emits side by side. `FORLOOP_I` is dynamically
      followed by the top of the loop body on every iteration and fusing
      them is not something the emitter can do — counting that pair would
      make the histogram argue for work nobody can perform. Pinned by
      `pairs_are_only_counted_when_the_two_are_neighbours`, where the
      back-edge contributes 0 pairs across 10 iterations and the
      fall-through exit contributes 1.

      **It is behind a `profile` cargo feature, and that is a measurement,
      not caution.** The counting loop is a second monomorphisation of the
      dispatch loop (`const PROFILE: bool`), so with profiling off there is
      no counter, no branch and no thread-local read on the hot path — and
      it was *still* 2–3% slower on `loop_arith`, `fib`, `array`, `closure`
      and `sort`. The second copy merely existing costs that much in code
      layout. Confirmed by building the same tree with the `true`
      instantiation unreferenced, which measured bit-for-bit at baseline,
      and by a control build of unchanged code, which measured identical to
      the shipped binary. A single loop with a runtime `bool` was worse
      again — up to **8.7%** on `loop_arith`, a branch per instruction being
      exactly what a dispatch loop cannot afford. So the default build pays
      **nothing**, and profiling is a rebuild away:

      ```bash
      cargo build --release --features profile -p saule-cli
      ```

      Asking for `--profile-bytecode` on a binary without the feature is an
      error naming the rebuild, never an empty report — an empty report
      reads as "your program executed no bytecode", which is a different and
      alarming claim. `saule_vm::profile::SUPPORTED` is what the CLI checks.

      **First results, on release builds.** `loop_arith`: `LOADI` and `MOVE`
      are **50%** of 40M instructions between them, and `MOVE LOADI` is the
      hottest pair — that is the deferred emission peephole (§17, "Peephole
      during emission"), not a superinstruction. `fib`: `MOVE` alone is
      **30%** of 11.8M, and `LTI TEST` runs 1,028,457 times as a pair even
      though `JLTI` — the fused comparison-and-branch — is already in the
      instruction set and already implemented. `disasm benchmarks/sau/fib.sau`
      shows another item off that same list in one screen: `0007 JMP -> 0008`
      is a jump to the very next instruction. Both readings say the same
      thing — the first Phase 5 wins are in the **emitter**, not in new
      opcodes. **Both were acted on; see the next item.** The `LTI TEST`
      reading in particular was not a request for a new instruction: `JLTI`
      already existed and was already implemented, and the profile's job was
      to notice that the compiler never emitted it.

      Two more worth recording, because neither is on the candidate list and
      both are larger than anything on it. `sort` spends **46%** of its 29M
      instructions in `CASTCHK` + `UNWRAPNIL`, 6,665,964 of each: reading a
      `table<integer, integer>` yields an optional that is checked and
      unwrapped on every comparison, and that is a *compiler* question about
      what a typed table read must prove, not a dispatch-loop one. `oop` is
      **42%** `MOVE` across 19M instructions, against 5.3% `CALLM` — the
      benchmark named for method dispatch spends eight times as much of
      itself shuffling registers as dispatching, which is worth knowing
      before the `CALLM` inline cache (§8.5) is written to speed it up.
- [x] **§17 emission peepholes — all of them, and the profile chose them
      over every candidate below.** Six changes across two slices, **no new
      opcodes and no dispatch-loop changes**: every one is the compiler
      emitting instructions the VM already had.

      Slice 1, the two largest:

      **(a) A comparison feeding a branch emits the fused form.**
      `binary_opcode`'s comment had claimed since Phase 1 that "the fused
      branch forms (§15.7) are used where the value feeds an `if`, which
      `stmt::cond_jump` handles". It did not. `JLTI` and its eleven siblings
      were implemented in the dispatch loop and **nothing ever emitted one**:
      every `if a < b` compiled to `LTI` + `TEST` + `JMP`, materialising a
      `Value::Bool` into a register read once and discarded. `--profile-bytecode`
      counted the `LTI TEST` pair 1,028,457 times in `fib` — that is what
      sent anyone looking.

      Gated on a **proved numeric kind**, which is what rules out an `Op*`
      overload without re-deriving `binary_to`'s contract lookup: an
      `integer` or `float` operand cannot be a class instance, so there is no
      `compare` or `equals` to dispatch to. An unproved `==` keeps `EQV` +
      `TEST`, and `an_unproved_equality_keeps_the_materialising_form` pins
      that it does. Float `==`/`!=` have no fused form and stay on the
      materialising path rather than switching to `JEQ`'s different
      predicate.

      **(b) An operand already in a register is used where it is.** `MOVE`
      was the most-executed opcode in every benchmark — 25% of `loop_arith`,
      30% of `fib`, 42% of `oop`, 26% of `sort` — and most of them were a
      local or a parameter copied into a fresh temporary purely to be an
      operand. `fib`'s `n < 2` emitted `MOVE 1 0` for an `n` that was sitting
      in register 0 the whole time; `oop`'s `self.y` emitted `MOVE r 0`
      2,000,002 times.

      **The safety rule is "every operand is pure", and it is not
      conservatism.** A captured local is an *open* upvalue pointing at this
      frame's register, so a closure called between the read and the use
      writes through it — and the operand would then read the new value
      where the tree-walker read the old one. Rather than track which locals
      are captured (a fact not even settled when the operand is compiled,
      because a lambda *below* it can capture), require that nothing is
      evaluated in between at all: every operand must be a literal, `self`,
      or a frame local. `n + f()` therefore still copies. Pinned by
      `an_in_place_operand_still_sees_the_value_the_oracle_sees`.

      `..` is excluded by name: `CONCAT` is n-ary over a register *range*, so
      its operands must be adjacent temporaries and reusing a local's
      register would break the range rather than shorten it.

      **Measured, release build, min of 7, ~27 ms of process start-up in
      every column:**

      | Benchmark | Before | After | Wall | Net of start-up |
      |---|---|---|---|---|
      | mandel | 170 ms | 144 ms | −15.3% | **−18.2%** |
      | oop | 187 ms | 161 ms | −13.9% | **−16.3%** |
      | fib | 108 ms | 97 ms | −10.2% | **−13.6%** |
      | closure | 100 ms | 92 ms | −8.0% | −11.0% |
      | loop_arith | 243 ms | 234 ms | −3.7% | −4.2% |
      | array | 137 ms | 135 ms | −1.5% | −1.8% |
      | sort | 705 ms | 700 ms | −0.7% | −0.7% |
      | map, strings, startup | — | — | flat | flat |

      And in instructions retired, which is the figure that is not a
      stopwatch:

      | Benchmark | Instructions | `MOVE` |
      |---|---|---|
      | fib | 11,827,257 → 8,741,887 (**−26%**) | 3,599,600 → 1,542,687 (**−57%**) |
      | loop_arith | 40,000,011 → 35,000,011 (−12.5%) | 10,000,003 → 5,000,003 (−50%) |
      | oop | 19,000,035 → 17,000,031 (−10.5%) | 8,000,014 → 6,000,010 (−25%) |

      `oop` moved from **3.1x** PUC Lua to **2.6x** on this box.

      **One in-place read was tried and is wrong, and the copy it leaves in
      is load-bearing.** `RET1` reading a local directly turns
      `fn run() local n = 0; local bump = fn() n = n + 1 end; bump();
      return n end` from `3` into `nil`, because `pop_frame` calls
      `close_upvalues(frame.base)` **before** it moves the results out, and
      closing does `mem::replace(slot, Value::Nil)`. The `MOVE` reads the
      register while the frame is still whole, which is exactly why it is
      correct. Caught by
      `a_closure_writes_through_to_its_captured_variable`, and now explained
      in place by `a_returned_local_is_still_copied_before_the_frame_pops`
      so nobody deletes it twice.

      **These tests assert the emitted code, not agreement.** A peephole that
      silently stops firing is invisible to every differential test in the
      file — the program still runs and still agrees, just slower — so the
      disassembly is the only thing that can catch the regression. Note the
      token-wise `emits()` helper: `contains("LTI")` is true of `JLTI`, which
      is the exact pair these tests exist to tell apart.

      **Slice 2 took the rest of §17's list.** Four more changes, still no
      new opcodes:

      **(c) A small integer literal folds into the instruction.**
      `ADDII` / `SUBII` / `MULII` take a signed 8-bit immediate, have been in
      the dispatch loop since Phase 1, and — the third instance of trap 9 in
      one week — **had never been emitted**. `loop_arith`'s
      `s = s + i * 2 - 1` spent two of its six instructions materialising
      `2` and `1` into registers read once. `Add` and `Mul` commute so the
      literal folds from either side; `Sub` does not (`1 - x` is not
      `x - 1`), and a literal outside `i8` keeps the register form rather
      than being truncated into a wrong answer.

      **(d) Arithmetic over pure operands is itself pure.** Slice 1's purity
      rule only admitted literals, `self` and frame locals, so `s + i * 2`
      still copied `s`: the right operand was a `Binary`. Arithmetic on
      proved-numeric operands cannot run user code — it is a typed opcode,
      not `ARITHX` — so it joins the rule. **The proved-kind condition is
      the whole of the safety argument**: without it the operator compiles
      to `ARITHX`, which calls `ops::binary`, which dispatches an `Op*`
      overload.

      **(e) `CASTCHK`, `UNWRAPNIL`, the unary ops and `GETIDX`/`SETIDX` read
      their operands in place**, the same way `GETF` already did. `sort`'s
      comparator — `(a as integer)! < (b as integer)!` on untyped lambda
      parameters — went from 8 instructions to 6.

      **(f) No jump to the next instruction.** `if_chain`'s loop carried the
      comment "only worth a jump to the end when something follows" and
      emitted one unconditionally, so every `if c then … end` ended in a
      `JMP` to the very next instruction. Decided at emission rather than by
      popping the instruction afterwards: popping would have to reason about
      handler `pc` ranges and the line table, while not emitting has nothing
      to undo.

      **(f) broke `json_usage`, and the bug is worth the space.** A proto
      gets a synthesized `RET0` when control can reach the end of its code
      array, and `pop_function` tested that with *"is the last instruction a
      return"*. Those are different questions: **a forward jump patched to
      `code.len()` lands one past the last instruction.** While every `if`
      arm ended in an unconditional jump the two coincided — so
      `fn f() … if c then return a end end`, whose last statement is a
      conditional return, ran off the end of the proto the moment those
      jumps stopped being emitted. `FuncCtx::max_patch_target` records the
      furthest target any jump was patched to, and `pop_function` asks that
      instead.

      **No fixture could have caught it** — it needs a function whose last
      statement is a conditional return, which is ordinary in real code and
      absent from small tests. `run_examples_diff.sh` caught it, on the one
      project shaped like a program. That is the fifth time this file has
      recorded the same lesson about fixture shape.

      **Measured, release build, min of 7, cumulative over both slices:**

      | Benchmark | Before Phase 5 | After | Wall | Net of ~27 ms start-up |
      |---|---|---|---|---|
      | loop_arith | 243 ms | 177 ms | −27% | **−31%** |
      | mandel | 167 ms | 137 ms | −18% | **−21%** |
      | fib | 107 ms | 90 ms | −16% | **−21%** |
      | oop | 187 ms | 159 ms | −15% | −17% |
      | closure | 101 ms | 91 ms | −10% | −13% |
      | array | 136 ms | 123 ms | −10% | −12% |
      | sort | 704 ms | 650 ms | −8% | −8% |
      | map, strings, startup | — | — | flat | flat |

      And in instructions retired, which is the figure that is not a
      stopwatch and not a loaded laptop:

      | Benchmark | Before Phase 5 | After | Change |
      |---|---:|---:|---:|
      | loop_arith | 40,000,011 | 20,000,011 | **−50%** |
      | mandel | 25,620,013 | 14,203,848 | **−45%** |
      | fib | 11,827,257 | 7,713,431 | **−35%** |
      | sort | 29,063,881 | 22,197,914 | −24% (a further −30% from `CASTUNWRAP`; see below) |
      | closure | 10,000,012 | 8,000,012 | −20% |
      | array | 11,000,017 | 9,000,017 | −18% |
      | oop | 19,000,035 | 17,000,029 | −11% |

      `loop_arith`'s inner loop is **6 instructions → 3**; `fib`'s hot proto
      21 → 15 with two fewer registers; `oop`'s constructor 7 → 3.

      **Still on the table, and now with a profile behind it:**

      * **`CASTCHK UNWRAPNIL` as a superinstruction.** 6,665,964 adjacent
        pairs in `sort`, 22.9% of the program in each half — which is
        exactly the justification §16 demands, and the first candidate this
        project has that meets it. Note what it is *not*: `sort` spends that
        time because the program says `(a as integer)!` on an untyped
        comparator parameter, and the tree-walker does the same work. The
        instruction count is a compiler artifact; the *cast* is the
        program's own semantics.
      * `MOVE r, r` → drop. Never observed being emitted; worth a debug
        assertion rather than a peephole until one turns up.
      * The remaining `MOVE`s in `fib` are call-result shuffles — the callee
        window's base differs from where the operand temp was allocated —
        which is a register-allocation question, not a peephole.
      * `map` and `sort` still barely move on the clock, for the reason §20
        gave before the VM existed: their time is inside `TableObject`.
- [ ] Inline caches for `GETFX` / `CALLIF`
- [x] Superinstructions from a measured opcode-pair histogram collected under
      `--profile-bytecode` (§16). **One shipped: `CASTUNWRAP`.** It was the
      only candidate that met §16's bar, and it was not on the list this item
      was written with.

      `(x as T)!` — `CASTCHK` followed immediately by `UNWRAPNIL`. The
      profile counted the pair **6,665,964 times** in `sort`, 22.9% of the
      program in each half. The compiler emits the fused form only from
      `Expr::ForceUnwrap(Expr::Cast { .. })`, at the `!`'s span, so a failed
      cast raises `ForceUnwrapNil` exactly where the pair did; a bare
      `x as T` keeps the nil-yielding `CASTCHK`, because the static type is
      `T?` and turning every failed cast into an error would be a language
      change. The cast itself still calls
      `saule_interpreter::eval::expr::cast::cast`, so the deep cases —
      `table<T>` elementwise, a class walking its chain — come along
      unchanged.

      **The result is the clearest evidence yet for §20, and it is worth
      more than the speedup.** `sort` retires **22,197,914 → 15,531,950**
      instructions, a **30% cut**, exactly the 6,665,964 the fusion removes.
      Wall clock moved **2.3%**. A thirty-percent instruction cut buying two
      percent says the remaining time is not dispatch: it is inside
      `TableObject` and in crossing the engine boundary once per comparison,
      which is what §20 predicted before the VM was written and what the
      `map`/`sort` rows have been saying since Phase 3.

      Read the 46% with its caveat, too: `sort` spends it because its
      comparator writes `(a as integer)!` on an untyped parameter and the
      tree-walker does the same work. The *pair* was a compiler artifact and
      is gone; the *cast* is the program's own semantics and is still
      performed.

      **Nothing else qualifies, and the original candidates least of all.**
      `GETF_CALLM`, `FORLOOP_GETARR`, `ADDII_MOVE`, `GETUPVAL_CALL` and
      `JLTI_ADDII` are unsupported by any reading, and two of them were
      written before the peepholes existed — `ADDII_MOVE` assumes a `MOVE`
      that no longer follows. Re-profile before reviving any of them, and
      expect the answer to be no: after the peepholes the top pairs are
      spread thin, which is what a compiler that stopped emitting redundant
      instructions looks like.
- [ ] `NativeClosureMulti` writing into `&mut [Value]`, for `stdlib/iter.rs`
- [ ] Precomputed hashes on constant string keys
- [ ] Raw `pc`/`base` pointers and `get_unchecked` in the dispatch loop —
      **only after the verifier lands**. It has, and its tests now cover
      every table it bounds — but read the note under "Verifier tests"
      before relying on it: `verify` runs under `debug_assertions` only, and
      the `EXTRAARG` payloads are still unchecked. `get_unchecked` on a
      chunk this compiler just produced is safe today because the compiler
      produced it; on a chunk read back from `.saule/cache/` it would not be.
      Sequence those two together.
- [ ] Dispatch threading experiments (worth 5–15%, cost real readability)
- [ ] Bytecode caching in `.saule/cache/` for `startup` on large projects
- [ ] **Only then:** reconsider NaN-boxing, with numbers. The decision today
      is no — `Value` is already 16 bytes, `i64` does not fit in 51 bits, and
      refcount traffic is the real cost (§4.2)

---

# Cross-cutting: testing

- [~] **Differential testing** — `crates/saule-vm/tests/differential.rs`
      runs every program under **both** engines and compares the result,
      error text included. **191 tests.** Closing the open correctness
      items added 12: a safe read, a safe call and a member **write** on an
      unproved receiver, a safe call's arguments **counted** to prove the
      nil guard still wraps them, a dynamic write reporting the same error
      as the tree-walker, an interface method's return type known at last
      (single- and two-valued, and one reached through `extends`),
      `return x?.m()` through both arms, and the reachability guard
      asserted in both directions — refusing a callee that reaches a later
      declaration, and *not* refusing an ordinary call with an unrelated
      `fn` below it. Tail calls added 12: a
      tail-recursive top-level `fn`, static method and lambda each recursing
      50 000 deep (constant depth is the assertion, so the depth has to
      exceed the 10 000 guard); mutual recursion, which proves the frame is
      reused rather than merely reset; a method call and a `return` inside a
      `try` asserted **on the disassembly** to be *not* tail calls, with a
      `catch` body asserted to still be one; a `try` that catches what a
      forced tail call throws (the interpreter bug); tail calls to a native
      and to a constructor falling back to ordinary calls; results passing
      through two replaced frames; upvalues closed by the frame a tail call
      replaces; and defaulted and variadic parameters bound through the
      entry stubs of a *dirty* reused frame. Multi-return added 16: both
      results of a call, a plain value list, nil padding in both directions,
      a surplus expression **counted** to prove it still runs, only-the-last
      expanding, the swap, parallel writes to fields and table slots,
      `return f()` passing everything through (the divergence), a
      single-valued callee *not* growing a second nil, a constructor
      returned through the same path, `CALLM_MR` beside plain `CALLM` in one
      program, an interface call's results through a `return`, a native's
      two results (`store_results`, a different padding path from
      `pop_frame`), module slots, a lambda callee, a driver whose body is a
      pass-through return, and eight values passing through a two-register
      frame. The `for … in` driver added 4,
      including a driver that yields nothing (the case that decided the
      calling convention) and nested driver loops.
 The `match`-guard fix added 3: a
      guard reading its own pattern's binding, a guard reading a
      destructured variant payload, and — the one that mattered — a
      *literal* pattern with a guard, the shape no fixture had and the
      reason a silent miscompile survived `SAULE_DIFF=1`.
      The re-entrancy slice added 8: a
      native invoking a bytecode comparator (ascending *and* descending, so
      a callback that is never consulted cannot pass), a comparator closure
      reaching a captured upvalue, a callback that itself re-enters the VM
      two levels deep — where a per-invocation field wrongly left in
      `VmShared` would corrupt the level below rather than merely be slow —
      a `toString` overload through `..` and `tostring` and *not* through a
      table, an overload counted to fire **exactly once** per operand, an
      overload resolved dynamically on a call result, an inherited
      `toString` reached through the flattened method map, and the
      recursion guard still unwinding after a sort's worth of callbacks.
      The nullability slice added 11:
      `??` laziness (counting the fallback's side effects rather than
      inferring), `!` on a value and on nil, `as` to each primitive, `as`
      to a class through an inheritance chain, `as table<T>` checked
      elementwise — the case a shallow `TypeDesc` would have got wrong —
      `?.` on a nil and a present receiver, a safe method call's arguments
      *not* being evaluated when the receiver is nil, a static read through
      an instance, and an inherited static read and written through a
      subclass name.
      Earlier: 19 tests covering literals, arithmetic (including
      i64 wrap-around and division by zero), comparisons, concatenation,
      `if`/`while`/`for`, nesting and block scoping. Programs the compiler
      cannot compile yet are *skipped*, since `Unsupported` is the designed
      fall-back signal, not a failure.
- [x] **`tests/ui/` audited** — all 144 fixtures read against their names,
      not just their exit status. Three were failing for the wrong reason;
      two are fixed and one is recorded as a missing diagnostic. See
      "`tests/ui/` is the diagnostic corpus" above and `tests/ui/README.md`.
- [x] **`SAULE_DIFF=1 ./run_tests.sh`** — every fixture under **both**
      engines, output compared character for character. This is the
      "conformance tests at no authoring cost" item, and it closes a hole
      that mattered more than it looked: the harness had only ever checked
      **exit status**, so a VM bug that printed the wrong value and exited 0
      passed silently. `OpToString` did exactly that.

      It found three divergences on its first run, none of which any
      existing test could see:

      | Divergence | Resolution |
      |---|---|
      | `#` on an integer: the VM wrote its own message (``` `#` is not defined for `integer` ```) instead of the tree-walker's | `Op::LEN` now defers to `ops::unary` for every non-length operand — reuse rather than reimplement, the same rule `ARITHX` follows |
      | Stack-overflow message names a different limit (10 000 vs 1 000 000) | **Intended** (§6.4: the VM counts frames and deliberately allows far more nesting). Exempted in `diff_exempt()` with the reason inline, and the exemption count is printed on every run so the list cannot grow unnoticed |
      | (the second `#` case, same cause) | same fix |

      A fallback note is stripped before comparing: the VM declining to
      compile something is a designed outcome, not a behavioural difference.
      **Run this alongside the other two modes** — plain `./run_tests.sh`
      and `SAULE_ENGINE=vm ./run_tests.sh` both pass on programs this one
      rejects.
- [x] Hand-assembled chunk tests for the implemented opcodes
- [ ] Encoding property tests with random operands
- [x] Verifier tests — hand-built malformed chunks must be *rejected*, not
      crash the VM. **Twenty-four now**, in `compile/verify.rs`'s own
      `mod tests` (counted with `grep -c '#\[test\]'`, not estimated).
      The seven that existed covered a register past the frame, a jump off
      the end, a constant index past the pool, a missing `EXTRAARG`, an
      undeclared upvalue, a proto that runs off its end, and a well-formed
      chunk. Added: an unassigned opcode byte, an orphan `EXTRAARG`, a proto
      with no instructions, more parameters than registers, and one
      out-of-range case **per table** `verify_proto` bounds.

      **Writing them found a hole, which is the point of writing them.**
      `GETMAPK` and `SETMAPK` were listed in the `Fmt::ABx` limit match and
      are **`Abc`** — so that arm never ran for them and their constant index
      had gone unchecked for the life of the verifier. A listing that reads
      as coverage and is not. The `Abc` path now bounds `GETMAPK`,
      `SETMAPK`, `JEQK`, `GETFX`, `SETFX`, `CASTCHK`, `CASTUNWRAP` and
      `CHKTY` against the table each one indexes.

      **And it found the opposite mistake immediately after.** `CALLMX`
      looks like `GETFX`'s sibling; its `C` is the **result count** and its
      member name rides in the `EXTRAARG`. Bounding `C` against the constant
      pool rejected three valid chunks, caught by the differential suite
      within a minute. `a_dynamic_member_call_is_not_mistaken_for_one` pins
      that shape so the next person extending the table has a reason not to
      add it back.

      A test per table, deliberately: the `_ => usize::MAX` arm means an
      opcode nobody listed is silently unchecked, and one test covering
      "some `Bx` is bounded" would not notice the next table arriving
      without a bound.

      **Still not verified, and now for a written-down reason:** the
      `EXTRAARG` payloads. Eleven opcodes take one and each packs something
      different — a module and proto packed 8/16, a constant index, a
      `dynop` code. `CALLMX` is the evidence that guessing from doc comments
      rejects valid chunks, which is worse than a gap. That wants a table on
      `Op` declaring what its `EXTRAARG` means, not a `match` written by
      hand.

      Two **positive** tests came out of it as well, both pinning rules that
      read like bugs at a glance: a `TAILCALLK` terminates a proto even
      though its `EXTRAARG` is the physically last word, and a `JMP` with a
      zero close-upvalues threshold is not a bad register.
- [ ] Closure-semantics fixtures asserting **values**, not just exit status:
      per-iteration capture, self-recursive locals, upvalue closing. This is
      where a subtle divergence is most likely and least likely to be caught
      by what exists.
- [ ] Memory fixtures from `PRODUCTION.md` §3.2 with recorded peak-RSS bounds,
      so 0.2 and 0.6 can be verified rather than assumed
- [ ] Benchmark regression gate with the ~3% noise floor in mind

---

# Open questions to settle as you go

| # | Question | Current recommendation |
|---|---|---|
| 1 | Move `Value` and the stdlib to a `saule-runtime` crate? | No, not now. `saule-vm` depends on `saule-interpreter`. Revisit only if the tree-walker is ever deleted (§24.7 Q1). |
| 2 | Delete the tree-walker after Phase 4? | No. It is the differential oracle. |
| 3 | Defaults: entry stubs or guarded prologues? | Stubs. Confirm with a microbenchmark in Phase 3. |
| 4 | `SWITCH`: dense table or binary search? | Dense for enums (tags are dense by construction), binary search for sparse integer matches. Compiler picks on density. |
| 5 | Reference cycles | The VM does not fix them (§24.3). What it changes is that GC roots become a small enumerable set — the register stack, module slots, open upvalues — which makes a future cycle collector a well-defined project instead of an open-ended one. |
