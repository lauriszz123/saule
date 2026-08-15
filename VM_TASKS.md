# Saule VM — Task List

> The execution plan for `VM_DESIGN.md`. That document is the *specification*;
> this one is the *checklist*. Section references (§) point back into it.
>
> **Ground rule, absolute:** `./run_tests.sh` passes at every commit, and the
> tree-walker stays the default engine until Phase 4.
>
> That means **all three modes**, because each catches what the others
> cannot:
>
> ```
> ./run_tests.sh                    # the tree-walker still works
> SAULE_ENGINE=vm ./run_tests.sh    # the VM runs or cleanly falls back
> SAULE_DIFF=1 ./run_tests.sh       # the two agree on *output*, not just exit status
> ```
>
> The third was added late and immediately found bugs the first two had been
> passing over for months — exit status alone cannot see a wrong value.

## Legend

| Mark | Meaning |
|---|---|
| `[x]` | done and tested |
| `[~]` | partially done — see the note |
| `[ ]` | not started |

## Where things stand

`crates/saule-vm/` exists and runs. The instruction encoding, the chunk
model, the disassembler, and the core of the dispatch loop are written and
covered by tests: hand-assembled chunks compute `1 + 2`, sum `1..100` through
a numeric `for`, recurse through `fib(20) = 6765`, capture an open upvalue,
build and index tables, and concatenate strings.

**Phases 0, 1 and 2 are complete.** The compiler turns Saule source into
bytecode and the VM runs it, 2.5x–3.9x faster than the tree-walker, with 43
differential tests asserting the two engines agree. `--vm` on `saule run`
falls back to the interpreter for anything the compiler does not reach yet,
so it is safe on any program.

**Phase 3 is in progress.** Classes, interfaces, enums + `match`,
`try`/`catch`, `for … in` (table path), operator overloading (left operand),
the `ARITHX`/`UNARYX` dynamic fallback, and **nullability** (`?.`, `??`,
`!`, `as`) are done. Remaining in §21.4 order: pipes, imports/modules, §19
argument binding — plus two blockers found while landing nullability, both
written up under Phase 3: **natives cannot call bytecode closures** (the
actual reason `sort.sau` still falls back) and `Assignable<T>`.

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
- [x] `op.rs` — all 115 opcodes from §15, `Fmt`, `Instruction` encode/decode
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
- [ ] `TAILCALL` — reuse the frame; closes the gap `PRODUCTION.md:344` names
- [ ] Variadic call/return through `top` (`B = 0` / `C = 0`) exercised by a test
- [ ] `SAULE_MAX_DEPTH` semantics documented as "frames", raised two orders of
      magnitude (§6.4) *(the limit is implemented; the doc change is not)*

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

- [ ] `TAILCALL` — needs no new front-end work; closes the gap
      `PRODUCTION.md:344` names
- [ ] Variadic call/return through `top` (`B = 0` / `C = 0`)
- [ ] `SAULE_MAX_DEPTH` re-documented as "frames" (6.4)

### Phase 2 exit criteria

- [x] Programs produce **identical results** under both engines —
      `crates/saule-vm/tests/differential.rs`, 43 tests, comparing values
      *and error text*
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

1. **Classes** — §8 *(in progress)*
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
   - [x] **8 of 9 benchmark files now run under the VM**, with output
         verified identical to the interpreter
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
   - [ ] `sort.sau` still falls back — but **not** on `!`, which now
         compiles. It falls back on `Table.sort`'s comparator: see
         "Natives cannot call bytecode closures" below.
   - [ ] Interfaces, and instance methods on a receiver whose class the
         front end did not prove

#### Measured: real benchmark files, interpreter vs `--vm`

Wall clock, min of 3, including ~90 ms of process start-up in both columns.
Output was checked identical before timing.

| Benchmark | Interpreter | VM | Speedup |
|---|---|---|---|
| fib | 386 ms | 169 ms | 2.28x |
| loop_arith | 682 ms | 306 ms | 2.23x |
| mandel | 516 ms | 234 ms | 2.21x |
| closure | 364 ms | 167 ms | 2.18x |
| array | 404 ms | 199 ms | 2.03x |
| oop | 510 ms | 256 ms | 1.99x |
| strings | 302 ms | 173 ms | 1.75x |
| map | 468 ms | 399 ms | 1.17x |

Net of start-up the ratios are roughly 2.5x-3.7x. `map` barely moves, exactly
as 20 predicts: it is dominated by hashing inside `TableObject`, which the VM
does not change.
   - [ ] Pass 1 layout: field slots and vtables as prefix-extensions of the
         parent's; order classes by inheritance depth
   - [ ] `NEW` with a constant `field_template` (one alloc + memcpy) and the
         `field_init` proto fallback
   - [ ] `GETF`/`SETF` static slots; `CALLM` vtable dispatch; `CALLSTAT`;
         `GETSTAT`/`SETSTAT` including the "write targets the *declaring*
         class" rule
   - [ ] `SUPER`, replacing the `SUPER_OWNER_BINDING` scope hack
   - [ ] **Single-source the layout** (§24.2): the runtime `ClassObject` is
         built *from* the `ClassProto`, not computed independently, and in
         debug builds `GETF` asserts the receiver's layout matches the one it
         was compiled against. A silent wrong-field read is the worst bug
         this project can ship.
   - [ ] Unlocks `oop.sau`
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
5. **`for … in`** — [ ] `ITERPREP`/`ITERNEXT`, both paths: the table
   snapshot (array then *sorted* map entries — that ordering is observable
   behaviour and must be preserved) and the closure driver, including
   `iter()` on instances
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
8. **Pipes** — [ ] lower `Expr::Pipe` to chained `CALLK`s
9. **Imports and modules** — [ ] per-module chunks, module slots,
   cross-module class layouts from the `ModuleSeed` registry
10. **Variadics, trailing blocks, named arguments, defaults** — [ ] §19.
    Decide entry stubs vs. guarded prologues with a microbenchmark (§24.7 Q3);
    stubs are the recommendation
11. **`ARITHX`/`UNARYX`** — [x] the dynamic fallback.
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

### The tree-walker cannot call into bytecode — one root cause, four symptoms

This is the single biggest thing left, and it is **not** a Phase 5
optimisation. `Value::VmFunction` is opaque to `saule-interpreter` on
purpose (§22.1), and a bytecode method is a `Proto`, not a
`FunctionObject` — so the runtime `ClassObject` the VM builds has an
**empty method map**. Every path where the tree-walker's own code needs to
call a user-defined function on a value hits that wall:

1. **Natives invoking a callable argument.** `Table.sort`'s comparator, and
   all of `stdlib/iter.rs`. Reports "value of type `function` is not
   callable". *This, not `!`, is what still makes `sort.sau` fall back.*
2. **`OpToString`.** `display_value` asks the class whether it has a
   `toString`, gets `no`, and falls back to `<instance of Money>` — **with
   no error at all**. Silently wrong output, the worst failure mode here.
3. **`ops::binary` / `ops::unary` overload lookup**, whenever the operand's
   class was not proved at compile time and the dynamic `ARITHX`/`UNARYX`
   fallback has to find the overload itself.
4. **A method read as a value** (`local f = obj.method`) has nothing to
   hand back.

Symptoms 1, 2 and 3-when-unproved are guarded by refusals today (see
below); each guard is a fallback that costs speed and none of them is
airtight. Fixing the root cause deletes all four guards at once.

**Recommended shape.** Split `Vm` into an `Rc<VmShared>` — chunk, module
slots, statics, enums, classes, closure cache — plus per-invocation state
(stack, frames, open upvalues). Give `VmFunction` a `call` method and
`call_value_multi` an arm for it; a callback then runs a **fresh** `Vm`
over the same shared half, and the existing `max_frames` limit bounds
recursion. Class method maps need to hold an erased callable rather than
`Rc<FunctionObject>`, which is the part that touches `saule-interpreter`.
Mechanical, but it reaches every `self.statics` / `self.module` in the
dispatch loop. Worth doing before Phase 4.

The guards in place until then:

#### Natives cannot call bytecode closures

`Value::VmFunction` is opaque to the tree-walker on purpose (§22.1):
`call_value_multi` has **no arm** for it. So a native that *invokes* its
argument — `Table.sort`'s comparator, and everything in `stdlib/iter.rs` —
reports "value of type `function` is not callable" the moment the VM hands
it a compiled closure. Nullability was expected to unblock `sort.sau`; it
compiles the `!` and the `as` now, and the file still falls back, on this.

Guarded for now rather than left to fail at run time: a **function-valued
argument to a native** is a clean `Unsupported`, so the program falls back.
Deliberately over-broad — a native that merely *stores* a closure would be
fine — because a needless fallback costs speed and a guess costs a wrong
answer. It catches a lambda literal or an argument the typechecker proved
is a `fn(...)`; a closure that reaches a native through an untyped local
still slips past, which is an argument for doing the real fix rather than
widening the guard.

The real fix is **VM re-entrancy**, and it is a slice of its own, not a
patch:

- `VmFunction` needs a `call` method, and `call_value_multi` an arm for it.
- A `Closure` today holds a proto and its upvalues — no VM. To run one it
  needs the chunk, the module slots, the statics and the class table, and
  `Vm::run` holds `&mut self` across the `CALLNAT` that would re-enter.
- Recommended shape: split `Vm` into an `Rc<VmShared>` (chunk, module
  slots, statics, enums, classes, closure cache) plus per-invocation state
  (stack, frames, open upvalues). A callback then runs a **fresh** `Vm`
  over the same shared half, and the existing `max_frames` limit bounds
  recursion. Nothing else needs to change; it is mechanical but touches
  every `self.statics` / `self.module` in the dispatch loop.
- Worth doing before Phase 4: `stdlib/iter.rs` is unusable from the VM
  until it lands, and that is not a corner of the language.

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

- [ ] All 91 `tests/*.sau` and all `tests/ui/*.sau` behave identically under
      both engines
- [ ] All 10 benchmarks run under `--vm`
- [ ] The differential harness is green across `examples/` and `www/`

---

# Phase 4 — Flip the default

*Estimate: 1–2 weeks.*

- [ ] `--vm` becomes the default; `--interp` selects the tree-walker
- [ ] `saule-wasm` switches `run` / `check_and_run` to the VM
- [ ] One release ships with both engines and a documented escape hatch
- [ ] Update `PRODUCTION.md` §"How fast is it?", the grade table, and
      Appendix A with **real measured numbers**
- [ ] `saule-lsp` and `saule-db` need no changes — confirm, don't assume (§14)
- [ ] Keep the tree-walker in-tree for at least one full release cycle. It is
      the differential oracle and it is ~13k lines that already work.

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
      error text included. **88 tests.** The nullability slice added 11:
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
- [ ] Verifier tests — hand-built malformed chunks must be *rejected*, not
      crash the VM
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
