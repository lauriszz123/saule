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

## Legend

| Mark | Meaning |
|---|---|
| `[x]` | done and tested |
| `[~]` | partially done — see the note |
| `[ ]` | not started |

## Where things stand

**Phases 0, 1, 2 and 4 are complete.** The compiler turns Saule source into
bytecode and the VM runs it 2.6x–3.7x faster than the tree-walker, with 191
differential tests asserting the two engines agree.

**The VM is the default engine** as of Phase 4. `saule run` uses it; `--interp`
or `SAULE_ENGINE=interp` selects the tree-walker, which stays in-tree as the
differential oracle. Nothing about the fallback changed — a module the
compiler cannot reach still runs on the tree-walker, silently now rather than
with a note, so the flip is safe on any program. `--vm` restores the note.

**Phase 4 did not change coverage, and coverage is the open risk.** 4 of the 9
comparable example projects still take the fallback, so for those the new
default is a no-op. Read Phase 4's "What flipping the default did *not* fix".

**Phase 3 is nearly complete.** Classes, interfaces, enums + `match`,
`try`/`catch`, `for … in` (table path), operator overloading (left operand,
including unary and index), **nullability** (`?.`, `??`, `!`, `as`), stdlib
value members, table dot access, the `ARITHX`/`UNARYX` dynamic fallback, and
**VM re-entrancy** are done. Remaining in §21.4 order: pipes,
imports/modules, §19 argument binding.

**Coverage, measured rather than inferred.** "236/236 under
`SAULE_ENGINE=vm`" counts a fallback as a pass, so it is not the number to
steer by. The real one:

| | Compiles fully | Falls back |
|---|---|---|
| `benchmarks/sau` | **10 of 10** | — |
| `tests/*.sau` | **84 of 92** | 8 |

(Re-measured at the Phase 4 flip. The line above said 85; `HEAD` was 84, and
the handoff before it made the same slip in the other direction. Re-count
before planning around either number — the census commands are below.)

**And the same measurement on real code, which says something different.**
`tests/*.sau` are single files; every real Saule program is a project with
imports, so the fixture ranking below is not the ranking that decides
whether the VM engages on anything a user would write.

| Corpus | Compiles fully |
|---|---|
| `examples/**/*.sau` | **10 of 61** |
| `examples/*` projects, end to end | **5 of 9 compared** (2 interactive, skipped) |

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

Cross-check its total against the exit-status count. If they disagree, trust
the exit status — a cause you cannot parse is still a refusal.

| Cause (first refusal per file) | Files |
|---|---|
| a class extending one the compiler cannot see | **24** |
| an import declaration | **10** |
| a name the resolver could not classify | 8 |
| a named argument to a callee the compiler cannot identify | 5 |
| a class implementing an interface this compiler cannot see | 2 |
| a variant of an unknown enum | 1 |
| a skipped parameter whose default must run in the callee | 1 |

First-refusal-wins, so a cause that only appears late in a file is
under-counted. The headline is now sharper than it was: **34 of 44
refusals are the cross-module slice** (24 + 10), which the fixture table
scores at *one* fixture. That is the gap between "236/236 with fallback"
and "the VM runs real programs", and it is now the only thing of its size
left.

Every remaining cause, by the fixtures it blocks. Regenerate this with:

```
for f in tests/*.sau; do SAULE_ENGINE=vm ./target/debug/saule.exe run "$f" 2>&1 \
  | grep -o "does not handle .*yet"; done | sort | uniq -c | sort -rn
```

| Cause | Fixtures | Note |
|---|---|---|
| an enum with methods | 1 | §0.6's missing `NodeId`, or a different key |
| a tuple pattern | 1 | |
| a skipped parameter whose default must run in the callee | 1 | §19 |
| a prelude name outside a call | 1 | |
| a declaration the compiler does not handle | 1 | |
| a compound assignment to a member | 1 | |
| a class implementing `Assignable` | 1 | |
| `self` outside a method | 1 | |

Each is independent; none unlocks another.

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
- [ ] Peephole during emission (drop `MOVE r,r`, fold small `LOADK` into the
      `*II` immediates, fuse comparison + branch, drop jumps to the next
      instruction). Deferred: each is a measurable optimisation, and 16
      says to measure first.

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

*Estimate: 4–6 weeks. In dependency order.*

1. **Classes** — §8. Done except the two items marked `[ ]` at the end.
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
   - [ ] A class from another module is refused rather than guessed — the
         imports slice lifts that
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
   [ ] Tuple patterns (`case (q, r)`) and nested payload patterns
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
6. **Operator overloading** — [ ] compile-time contract resolution via
   `binary_contract`; dispatch-on-left-operand and the `==`/`compare`
   symmetry rules move into the compiler. `..` falling through to
   `OpToString` needs care (§8.7)
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

   - [ ] `?.` on a receiver whose class the front end did not prove, and on
         a table or string receiver, still refuses — it needs the dynamic
         `GETFX`, which is Phase 5's inline-cache work
   - [ ] Tuple/`Nullable` `catch` types still collapse to `TypeDesc::Any`;
         unchanged by this slice

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
| `for … in` over an **unproved** source | 2 |
| an import of a dynamic native package | 2 |
| a variadic or defaulted parameter | 1 |
| a member read on a receiver with no proved class | 1 |
| a class implementing `Assignable` | 1 |

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
    - [~] A skipped parameter that has a **default**. Two different cases,
          and only one of them works:
          * A default at the **end** of the parameter list, filled by the
            entry stubs, does run in the callee: a default of `nextId()`
            called twice yields `a#1`, `b#2`, `calls=2` under both engines,
            so it is evaluated per call rather than folded once at the call
            site. Pinned by `a_default_is_evaluated_in_the_callees_frame`.
          * A default **skipped in the middle** still refuses, at
            `compile/expr.rs`'s `reorder_args`, with `a skipped parameter
            whose default must run in the callee`. Stubs fill a *suffix*;
            there is no entry point meaning "fill slot 1 but not slot 2".

          **This item was marked `[x]` and claimed `trailing_block_layout.sau`
          compiles. It does not** — `saule disasm tests/trailing_block_layout.sau`
          exits 1 on `Ui.panel(title: "inner") do … end`, which skips the
          defaulted `pad` while filling the `body` that follows it. The cited
          test passes and is real; it covers the trailing case, which is the
          one the stubs already handled. Fixing the middle case wants either
          a per-gap-pattern stub or a sentinel the prologue tests.
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
outright. **It refuses nothing in either corpus**: 84 of 92 fixtures and 17
of 61 example files compile exactly as before.

The canary `a_forward_reference_reached_through_a_callee_still_diverges` is
repointed and renamed; it now asserts the refusal, plus the two fixtures the
blunt guard broke as the other half of what it is worth.

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

`SAULE_ENGINE=vm` is back to **235/235**.

| Was failing | Cause | Fix |
|---|---|---|
| `shapes`, `fn_type_variance` | **Inherited vtable slots were never filled.** Pass 1 copies the parent's vtable so the slot *numbering* extends it — but no body is compiled yet, so what it copies is a row of `u32::MAX`, and `class_decl` fills only the slots a class declares itself. `Circle.describe`, inherited and not overridden, stayed unfilled. | Pass 2a: one forward sweep after codegen, parents before children (which `order_by_depth` already guarantees), filling any slot still `u32::MAX` from the parent. |
| `operator_overload` | `compare` and `equals` return a *value*, not the operator's answer. The overload path used the raw result, so `b < a` evaluated to `-180`. Unary `-` had no compile-time path at all. | Post-process the way `ops::binary` does — `compare` read against zero for all four orderings, `equals` normalised through `NOT`/`NOT` and negated for `!=`. Unary overloads resolved at compile time like the binary ones. |
| `op_index` | `GETIDX`/`SETIDX` are table-only despite §15.9 calling them the dynamic form, so an instance receiver hit "expected `table`". | `OpIndex`/`OpNewIndex` resolved to vtable slots at compile time, same pattern. |
| `iter_closure`, `iter_object`, `iter_pairs`, `ui/iter_missing_iter_method` | `ITERPREP` emitted for a `for … in` over a closure or an instance — the closure-driver path (item 5) is not written. | Refuse unless the source is a **proved table**. An unproved table is refused too; that costs a needless fallback, which is the right side to err on. |
| `ui/implements_missing_method` | A class missing an interface method compiled with a hole in its itable. Nothing before the compiler rejects it — the *tree-walker* catches it at class declaration. | Pass 1 refuses when a declared interface has an unmatched method; a new pass does the same for the stdlib contracts, looked up by name in a prelude scope exactly as the tree-walker looks them up. |

Not fixed, refused instead — the closure-driver `for … in` (item 5) and
right-operand operator dispatch (item 6) are still real gaps. What changed
is that they now fall back rather than compute a wrong answer.

### Phase 3 exit criteria

- [x] All `tests/*.sau` and all `tests/ui/*.sau` behave identically under
      both engines — `SAULE_DIFF=1 ./run_tests.sh`, 235/235, output compared
      rather than just exit status, one documented exemption.
      **Note what this does and does not say:** it holds *including* the
      programs that fall back, so it proves the two engines agree, not that
      the VM compiles everything. Read it together with the coverage table
      at the top.
- [x] All 10 benchmarks run under `--vm` — `sort.sau` was the last, and
      re-entrancy is what unblocked it.
- [~] The differential harness is green across `examples/` —
      `run_examples_diff.sh`, **9 of 11 projects compared, both engines
      agreeing on every one**. `www/` is not covered yet.

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
- [ ] Coverage: 85 of 92 `tests/*.sau` compile fully. The remaining 7 fall
      back cleanly, which is correct-but-slow; the gaps are listed at the
      top of this file.

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
      §3.3, §3.6 and §10's Phase 6 were rewritten too — each of them argued
      *for* building a VM, and reads wrong once one exists.
      The LuaJIT column was **removed** rather than reused: it is not installed
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

*Ongoing, and **only with a profile in hand**.*

- [ ] Inline caches for `GETFX` / `CALLIF`
- [ ] Superinstructions from a measured opcode-pair histogram collected under
      `--profile-bytecode` (§16). Candidates in expected-value order:
      `GETF_CALLM`, `FORLOOP_GETARR`, `ADDII_MOVE`, `GETUPVAL_CALL`,
      `JLTI_ADDII`
- [ ] `NativeClosureMulti` writing into `&mut [Value]`, for `stdlib/iter.rs`
- [ ] Precomputed hashes on constant string keys
- [ ] Raw `pc`/`base` pointers and `get_unchecked` in the dispatch loop —
      **only after the verifier lands**
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
- [~] Verifier tests — hand-built malformed chunks must be *rejected*, not
      crash the VM. **Seven exist**, in `compile/verify.rs`'s own `mod
      tests`: a register past the frame, a jump off the end, a constant
      index past the pool, a missing `EXTRAARG`, an undeclared upvalue, a
      proto that runs off its end, and a well-formed chunk that passes.
      (An earlier note here said none existed; it was wrong.) What is *not*
      covered: a bad opcode byte, an `EXTRAARG` with no instruction before
      it, and an out-of-range `Bx` for each of the tables `verify_proto`
      limits.
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
