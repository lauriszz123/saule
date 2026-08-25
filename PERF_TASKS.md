# Saule VM — Performance Task List

> Closing the remaining gap to Lua. `VM_TASKS.md` is the feature checklist;
> this one is the *speed* checklist, and every item on it is justified by a
> profile rather than by intuition.
>
> **Ground rule, inherited from `VM_TASKS.md` and absolute:** all four test
> modes pass at every commit, and the tree-walker stays green — it is the
> differential oracle.
>
> ```
> ./run_tests.sh                       # the VM, the default engine
> SAULE_ENGINE=interp ./run_tests.sh   # the tree-walker still works
> SAULE_ENGINE=vm ./run_tests.sh       # the VM runs or cleanly falls back
> SAULE_DIFF=1 ./run_tests.sh          # the two agree on *output*
> ./run_examples_diff.sh               # multi-module projects, both engines
> python3 benchmarks/bench.py check    # still agrees with Lua
> ```

## Where we are

Measured against Lua 5.5, min-of-9 interleaved, geomean excluding `startup`:

| | session start | now |
| --- | --- | --- |
| **geomean** | **1.73x** | **1.30x** |
| `map` | 1.20x | **0.46x** |
| `strings` | 1.80x | 1.02x |
| `mandel` | 1.79x | 1.10x |
| `wordfreq` | 1.56x | 1.18x |
| `json` | 1.50x | 1.22x |
| `closure` | 3.00x | 2.00x |
| `fib` | 3.09x | 2.43x |

Landed so far: table growth 2x→3x (`TableObject::grow_hint`), integer
rendering without `core::fmt` (`saule_interpreter::itoa`), single-buffer
concatenation (`display_into`/`display_hint`), and four fat dispatch arms
moved out of line.

## The profiles these tasks are built on

Leaf-weighted, `sample`, release build. **Do not re-measure these to start a
task** — they are recorded here so the next person doesn't have to.

**`fib`** — the worst row, and it is all call machinery:

```
execute_loop              59.0%
push_frame_resolved       12.9%   ┐
enter_static              12.6%   │  37% call machinery
pop_frame                  9.7%   │
drop_in_place<Frame>       2.4%   ┘
drop_in_place<Value>       3.8%
```

**`map`** — the string/table half:

```
execute_loop               474      TableObject::set           385
TableObject::get_str       251      free                       242
malloc                     211      memcmp                     127
reserve_rehash             123
                                    → ~35% allocator, ~31% table
```

## The refcount question, answered early so nobody re-opens it

Lua does not refcount: a `TValue` copy is a 16-byte move with no
bookkeeping. Saule's is an increment now and a decrement plus a zero-test
later, on every move that touches a heap variant. It is tempting to call
that *the* structural gap and reach for a garbage collector.

**The profiles say no.** Refcount traffic is ~6% of `fib`
(`drop_in_place<Value>` 3.8%, `drop_in_place<Frame>` 2.4%) — and `fib`'s
loop holds nothing but integers, which are never refcounted at all. The two
rows furthest from Lua, `fib` and `closure`, would barely move.

Task 2 makes the same point from the other side: two separate attempts to
take `Rc`s off the call path both *lost* ~10%, because what replaced them
was worse than what they cost.

So refcounting is not the wall, and this file is deliberately ordered that
way — the memory-management item (Task 4) is filed as a **correctness** fix
for a demonstrated leak, and the speed items go after hashing (Task 1, done)
and dispatch (Task 5).

## Two constraints that bound every design here

1. **`Value` must stay 16 bytes.** Measured by padding a variant and
   re-running the suite: 32 bytes costs **~2% geomean**, 24 bytes costs
   **~4%** (24 is not a power of two, so `Vec<Value>` indexing needs a
   multiply instead of a shift). Any new string representation must fit in
   an 8-byte payload — which rules out inline short-string storage and
   `Rc<str>` alike.
2. **`execute_loop` is dominated by its own code size.** This has now paid
   out three separate times: the `profile` feature's second copy (2-3%,
   recorded in `Cargo.toml`), removing a `Vec<String>` from `CONCAT`
   (double-digit percentages on benchmarks that never concatenate), and
   outlining four allocating arms (-4.8% geomean, every row improved). Any
   change here must be measured against the **whole suite**, never against
   the benchmark it targets.

---

## Task 2 — Frames by index, not by `Rc`

**Targets** `fib` (2.31x), `closure` (1.90x), `entity` (1.72x), `sort`
(1.59x). **Attacks** the 37% of `fib` above.

A `Frame` holds three `Rc`s — `func`, `proto`, `chunk` — so a call is three
increments and a return is three decrements with three zero-tests. It is
48 bytes pushed and popped per call.

None of that is necessary. Replace the pointers with the indices they were
resolved from:

```rust
struct Frame {
    module: u16,      // -> shared.chunks[module]
    proto_idx: u16,   // -> chunk.protos[proto_idx]
    base: u32, ret_to: u32, pc: u32, top: u32,
    n_ret: u8, n_args: u8,
}
```

48 bytes → 20, and **zero refcount traffic**. Proto and chunk are recovered
by two indexed loads at frame *activation* — once per call and once per
return, not per instruction.

`func` is the only real obstacle: three upvalue opcodes read it. But a
statically-resolved call — all of `fib`, and most real code — has its handle
living forever in `shared.closure_cache[module][proto_idx]`, reachable from
the same two indices the frame now stores. Only a closure carrying upvalues
needs a reference of its own:

```rust
enum FrameFunc { Static, Closure(Rc<VmFunctionRef>) }
```

### ✗ ABANDONED — both designs measured, both are losses

**Do not attempt a third variant without reading this.**

| attempt | geomean | `fib` | `closure` |
| --- | --- | --- | --- |
| `Frame.proto`/`chunk` as `ManuallyDrop<Rc<_>>` | **+10.4%** | +17% | +30% |
| `Frame` by `(module, proto_idx)`, 48B → 32B | **+11.1%** | +25% | +43% |

Two structurally different designs, the same regression, the same shape —
worst on exactly the call-heavy benchmarks they were built to help. That is
not noise, and the second run makes the mechanism clear.

**The frame's `Rc`s are not the cost; re-deriving what they point at is.**
`execute_loop` re-derives proto and chunk once per frame *activation* — and
an activation happens on every call **and every return**, so `fib` does it
~60M times. With the `Rc`s in the frame that is two loads from a cache line
that was written moments ago. With indices it becomes a dependent chase:

```
self.shared -> VmShared -> chunks Vec ptr -> [module] -> Chunk -> protos Vec ptr -> [idx]
```

Six serialized loads, each waiting on the one before. Three refcount
increments are *independent*, pipeline freely, and hit L1 — they lose to
pointer-chase latency by a wide margin. Removing work is not the same as
removing time.

**What this says about the profile.** `fib`'s 37% in the call machinery is
real, but it is not refcount traffic. It is register-file setup
(`claim_registers`), the missing-parameter nil fill, the 48-byte frame push
and pop, `entry_for`, and the `max_frames` check — which reads through
`self.shared`, a pointer chase of its own on every call and a candidate for
caching in `Vm` directly. Anyone returning to this task should price those,
and leave the three `Rc`s alone.

---

## Task 1 — Interned strings with a cached hash ✓ DONE

**Measured: -4.3% geomean, `map` -42.0%, `wordfreq` -6.0%, `json` -3.3%, no
regressions.** `map` went from 0.83x Lua to **0.46x** — better than twice
Lua's speed on string-keyed table work.

Landed as `saule_interpreter::value::str::SauleStr`, replacing
`Value::Str(Rc<String>)` across ~120 call sites. `Deref<Target = String>` is
what kept that tractable: `**s`, `(**s).clone()` and `s.len()` all still mean
what they meant. The existing intern table in `stdlib/string.rs` now hands
out `SauleStr`, so interned strings share a hash as well as an allocation,
and `EnumVariantObject`'s two name fields became `SauleStr` so reading
`.name` off an enum value is a refcount bump rather than an allocation.

The hash is **lazy** — `Cell<u32>`, `0` meaning "not yet" — so
`benchmarks/sau/strings.sau`, which builds 200k strings and uses none as a
key, pays nothing. `TableKeyRef::Str` carries `(text, hash)` so both probe
paths agree bit for bit, and `TableKey::from_value` clones an `Rc` where it
used to deep-copy a `String` on **every map insert**.

### The original plan, for reference

**Targets** `map`, `strings`, `wordfreq`, `json`, and every record-shaped
table. **Attacks** the ~35% allocator and ~31% table time above.

`Value::Str(Rc<String>)` is **two allocations** — the `RcBox`, and `String`'s
byte buffer — and two pointer hops to reach a byte. Nothing caches the hash,
so `TableObject::get_str` re-walks the bytes on every lookup and
`reserve_rehash` re-walks them again on every growth.

Lua interns short strings and caches the hash in the `TString` header. That
is why `map` and `wordfreq` were close to begin with.

```rust
struct StrHeader { strong: Cell<u32>, hash: u32, len: u32, /* bytes inline */ }
Value::Str(NonNull<StrHeader>)   // thin pointer — Value stays 16 bytes
```

One allocation, one hop, hash is a load. Equality tries the pointer first
(interned literals answer there), then the hash, then the bytes.

What it should reach:

- `TableObject::set` + `get_str` — hashing becomes a `u32` load
- `reserve_rehash` — reads the stored hash instead of re-walking
- allocator traffic — halved
- `TableKey::Str(String)` — currently a **deep string copy on every map
  insert**; becomes a refcount bump

A standalone reproduction of `map`'s inner loop put the thin representation
at roughly **17 points beyond the itoa win**.

### Measured dead ends — do not re-explore

- **Inline short strings** (`[u8; 22]`): -0.0%, +1.1%, -3.3% across three
  runs — indistinguishable from zero — against the ~2% `Value`-widening
  cost it forces. Net loss.
- **`Rc<str>`**: +19% on the same workload. A fat pointer *and* it copies
  bytes on construction.
- **Caching the hash inside `TableKey`** rather than the string: worth ~2%
  on top of the 3x growth change, which does not pay for reshaping a `pub`
  enum that also carries the `Ord` iteration-order contract.

---

## Task 3 — Move the remaining per-execution work into the compiler ~ PARTLY DONE

- **Pre-hashed table keys — ✓ delivered by Task 1, for free.**
  `GETMAPK`/`SETMAPK` index `chunk.constants`, and a constant is now a
  `SauleStr`. Its hash is therefore computed once for the entire program run
  and read on every access after. No opcode or chunk-format change was
  needed; this is most of `wordfreq`'s -6.0% and `json`'s -3.3%.
- **`NEWVAR`'s two `String` allocations — ✗ not worth doing.** The field
  types are now `SauleStr`, but `new_variant` still builds them from
  `chunk.enums[..].name.to_string()`. Making that free means pre-building
  the names in the chunk, which touches all seven `EnumObject` construction
  sites — **for zero measurable benefit**, because `NEWVAR` only fires for
  *tuple* variants and every enum in the benchmark suite (including
  `interp.sau`, the enum-heavy one) uses unit variants, which come from
  cached singletons. Do it when a benchmark exercises it, not before.
- **`entry_for(n_args)` per call — ✗ not attempted.** It is on the call
  path, and Task 2 is two pieces of evidence that this area punishes
  changes. Price it against the whole suite before believing anything.
- **`CALLMX`'s constant clone — ✗ not attempted.** Now a refcount bump
  rather than a `String` copy anyway, and the opcode is rare.

### The original plan, for reference

The constant-pool principle: if the compiler knows it, the VM should not
compute it. Rides along with Task 1's interned pool.

- **Pre-hashed table keys.** `GETMAPK`/`SETMAPK` already take a *constant
  index*. Make that constant a pre-built `TableKey` carrying its hash, and
  literal-key access skips both the string clone and the hash entirely.
  `wordfreq`, `json` and record-shaped code are the beneficiaries;
  `map.sau` is not, because its keys are computed.
- **`NEWVAR` allocates a `String` per execution** —
  `chunk.enums[e].variants[tag].name.to_string()`, plus `e.name.clone()`.
  Both are compile-time constants and belong in the interned pool.
- **`entry_for(n_args)`** is resolved per call, but arity is static at each
  call site; it can be folded into the call instruction.
- **`CALLMX`** clones `chunk.constants[k]` for a name the compiler knows.

---

## Task 4 — A cycle collector (correctness, not speed)

**`Rc` cannot collect cycles, and Saule leaks them today.** 200,000 tables
in a loop, release build, VM engine:

```
t["pad"] = "…"                 →   4 MB peak RSS
t["self"] = t;  t["pad"] = "…" →  50 MB peak RSS
```

Unbounded, and it is the user's `t.self = t` that does it — not an exotic
shape. Any doubly-linked structure, any parent↔child instance graph, any
closure that captures the thing holding it. For a game loop or a long-lived
script this is a defect rather than a benchmark number.

This is already known here in the specific: `Closure::shared` is `Weak` on
purpose to break one cycle, and `closure_semantics.rs` guards another. Those
are hand-patched instances of a general hole.

### Do trial deletion on top of `Rc`, not a tracing GC

Bacon–Rajan trial deletion, the approach CPython uses:

- Only the container variants can form a cycle — `Table`, `Instance`,
  `EnumVariant`, and `Closure`/upvalues. `Str`, `Int`, `Float`, `Bool` and
  the native handles cannot, so most of `Value` is untouched.
- Buffer "candidate roots": objects whose refcount is decremented to
  something still non-zero.
- Periodically, over *only* those candidates, decrement internal references
  to find subgraphs unreachable from outside, then free them.

**Why this shape and not a real collector.** Two things make a tracing GC
expensive here specifically, and trial deletion sidesteps both:

1. **The native ABI.** `saule-native-abi`, `saule-sdk` and the
   `libloading` dynamic packages hand `Value`s to foreign code. Refcounting
   keeps those alive for free. Tracing would need every native to root what
   it holds — a breaking ABI change for every package, and an API whose
   misuse produces use-after-free rather than a clean error. Trial deletion
   changes nothing: natives keep their `Rc` and stay correct.
2. **The tree-walker's roots live on the Rust stack.** `eval` calls `exec`
   calls `eval`, recursively, with `Value`s in Rust locals that a precise
   collector cannot see — so tracing would need a shadow stack (invasive
   and slow, in the engine that is the differential oracle) or conservative
   stack scanning (fragile). Trial deletion needs neither: those locals are
   real strong references and stay counted.

No safepoints, no rooting, no ABI change. It is additive.

### What it is *not* for

**Not a performance task.** Refcount traffic is ~6% of `fib`
(`drop_in_place<Value>` 3.8% + `drop_in_place<Frame>` 2.4%), and `fib`'s
loop holds nothing but integers, which are not refcounted at all. A
collector would not move `fib` or `closure` — the two rows furthest from
Lua — measurably. Anyone reaching for GC to close the speed gap is reading
the wrong profile; see Task 5.

---

## Task 5 — Replicated dispatch

**Targets `fib` (2.43x) and `closure` (2.00x)** — the two rows Tasks 1-3 did
not move, and the ones a collector will not move either.

Lua dispatches with computed goto: the decode-and-jump is **replicated at
the end of every opcode handler**, so each opcode gets its own
branch-predictor entry. Rust's `match` compiles to a single indirect branch
shared by all ~100 opcodes, which predicts poorly on a varied instruction
stream. On a tight loop that is a structural disadvantage, and it is the
most plausible remaining explanation for `fib`.

Stable Rust cannot express guaranteed tail calls (`become`, RFC 3407, is
unstable), so the approximation is to duplicate the decode-and-dispatch tail
into the hottest arms so they branch straight to their successor rather than
back to the shared head.

### Handle with the care this loop has earned

`execute_loop` is dominated by its own code size — recorded three times over
in this file and in `Cargo.toml`. Replicating dispatch makes the function
substantially *bigger*, which is exactly the axis that has produced both the
largest wins and the largest regressions of this whole effort. It could pay
double digits or cost double digits.

So: replicate into a **small** set of arms first — the ones
`--profile-bytecode` shows carrying `fib` and `closure` — measure against
the whole suite, and widen only while the geomean keeps improving. Do not
convert all hundred arms and then measure.

---

## Expected outcome, and the ceiling

Task 1 has landed and moved the string and table half decisively (`map` is
now **0.46x** Lua). Task 3's remaining items are small. Task 2 is closed as
a loss.

What is left is Task 5: the 1.22–1.40x cluster is respectable and probably
near what this dispatch design gives, while `fib` and `closure` are held
back by the shared indirect branch. If replicated dispatch works, ~1.1x
geomean is plausible. If it does not, this VM is at its design's floor and
the next move is a different dispatch architecture, not another
optimization.

Task 4 is orthogonal to all of it and should be scheduled on its own merits:
it buys no speed and fixes a real leak.
