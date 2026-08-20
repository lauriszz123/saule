# Saule — Production Readiness Analysis

An assessment of what Saule is today, what it would take to become a language
ordinary people use for real work, how you would *know* it is stable, how hard
it is to embed in another project, how hard it is to write a native library for
it, and how the code should be split across crates and repositories.

Everything below was originally measured against the working tree at `46e810e`
(main, 2026-08-10). Numbers were produced by running the suites and benchmarks
on this machine (macOS 25.6, arm64), not estimated.

> **Revised 2026-08-12 against `197ef4f`.** Parser error recovery — ranked as the
> single highest-leverage gap in the original draft — has since landed, and the
> sections that turned on it are rewritten below. The memory-model section
> ([§3.2](#32-memory-model)) is substantially rewritten twice over: the cycle
> leak turned out to be materially broader than the original draft described,
> and the dominant case — every closure stored in a local — has since been
> fixed. Both the problem and the fix are backed by measurement rather than by
> reading the types. Crate LOC counts and the language benchmark table predate
> `197ef4f` and have not been re-run.

> **Revised 2026-08-13.** Two items from §9.2 have landed and their sections are
> rewritten: `saule-project` now owns `saule.config` for all three consumers, and
> `saule-db` is the incremental query layer for parse / imports / seed. Both
> sections correct a claim the earlier drafts made without checking — see the end
> of each. Crate LOC counts and the language benchmark table still predate
> `197ef4f`.

> **Scope note.** This document does not repeat [RELEASE_PLAN.md](RELEASE_PLAN.md),
> which already sequences distribution and the package manager in detail and is
> largely correct. This is the layer above that plan: what the plan does *not*
> cover, and whether shipping it would actually be enough.

---

## Table of contents

- [1. The verdict in one page](#1-the-verdict-in-one-page)
- [2. What was measured](#2-what-was-measured)
- [3. How the language behaves](#3-how-the-language-behaves)
  - [3.1 Type system](#31-type-system)
  - [3.2 Memory model](#32-memory-model)
  - [3.3 Execution model](#33-execution-model)
  - [3.4 Error model](#34-error-model)
  - [3.5 Module and project model](#35-module-and-project-model)
  - [3.6 Concurrency model](#36-concurrency-model)
- [4. Scorecard against a production language](#4-scorecard-against-a-production-language)
- [5. The gaps, ranked](#5-the-gaps-ranked)
- [6. How to know it is stable](#6-how-to-know-it-is-stable)
- [7. Is it easy to embed?](#7-is-it-easy-to-embed)
- [8. Is it easy to write a native library?](#8-is-it-easy-to-write-a-native-library)
- [9. Crate and repository topology](#9-crate-and-repository-topology)
  - [9.2.1 What blocks the rest of the query layer](#921-what-blocks-the-rest-of-the-query-layer)
- [10. Sequenced roadmap](#10-sequenced-roadmap)
- [Appendix A — raw measurements](#appendix-a--raw-measurements)

---

## 1. The verdict in one page

**Saule is a well-built language implementation and not yet a product.**

The engineering quality is genuinely high, and that is not a courtesy: 74k lines
of Rust across 16 crates, 847 passing Rust tests plus 224 `.sau` fixtures, zero
failures, near-zero `TODO`/`FIXME` markers, blocking `clippy -D warnings` and
`rustfmt` gates in CI, and module-level documentation that explains *why* rather
than restating the code. The type system is more coherent than most hobby
languages — no bare `function` type, invariant table elements, an `any` you must
cast out of, split `integer`/`float` with no implicit promotion. The native
package ABI is frozen, documented, and paired with a proc-macro SDK that infers
the Saule signature from Rust types so the manifest cannot drift. That is
better than what most languages have at 1.0.

What is missing is almost entirely the *product* layer, and it is missing
uniformly:

| Question a new user asks | Answer today |
|---|---|
| How do I install it? | Clone the repo and build it with Rust. One git tag exists; no published artifact. |
| How do I add a library? | You cannot. `dependencies:` accepts local relative paths only. |
| How do I write a test? | There is no test runner. Write a `.sau` file and check the output by eye. |
| Where did my error come from? | The line it failed on. There is no call stack. |
| Is my editor going to work? | Yes — the parser recovers, so features survive half-typed code. |
| How fast is it? | 0.8×–4.8× PUC Lua on the bytecode VM, now the default engine, and **1.24×–3.52× the tree-walker** — the second figure is the one that does not depend on which Lua is installed. Phase 5's peepholes cut instructions retired by 11–50% depending on the benchmark. |
| Is it stable? | Nothing states what "stable" means, and nothing enforces it. |

None of those are research problems. All of them are work, and the sequencing
matters more than the total volume — see [§10](#10-sequenced-roadmap).

The three things that most change Saule's trajectory, in order:

1. **Shipping a release at all.** The release workflow builds six triples and has
   never published; one tag exists. Every other item on this list is invisible
   until someone who is not the author can install `saule`. This is now the
   binding constraint, and it is process, not engineering.
2. **Publishing `saule-native-abi` / `saule-sdk` / `saule-export-macro` to
   crates.io.** Today they are path dependencies inside this workspace, so
   "write a native library for Saule" means "clone the compiler." That is a hard
   stop on third-party native packages, which is the ecosystem's growth path.
3. **A stability contract.** Not a version number — a written statement of what
   may change and what may not, with tests that fail when it does. §6 proposes
   one.

> **Done since the original draft.** Parser error recovery was the first item on
> this list and has landed. `saule_parser` now exposes `parse_recover` and
> `parse_recover_with_prior` alongside the strict `parse`
> ([lib.rs:157](crates/saule-parser/src/lib.rs:157)), built on a 575-line
> [recover.rs](crates/saule-parser/src/recover.rs) and covered by 33 tests in
> [recovery_tests.rs](crates/saule-parser/tests/recovery_tests.rs). The LSP
> consumes it through [syntax.rs:56](crates/saule-lsp/src/syntax.rs:56), and
> carries the good idea of seeding recovery from the last *clean* parse of the
> file ([server.rs:105](crates/saule-lsp/src/server.rs:105)) so the recovered
> tree has fewer holes. Completion additionally tries a repair pass before
> falling back to the recovered tree
> ([completion/repair.rs](crates/saule-lsp/src/server/completion/repair.rs)).
> This removes the largest single gap between "an editor plugin exists" and
> "the editor feels good".

---

## 2. What was measured

| Measurement | Command | Result |
|---|---|---|
| Rust test suite | `cargo test --workspace --exclude saule-engine-lib` | **847 passed, 0 failed, 5 ignored** |
| Language fixtures | `./run_tests.sh` | **224/224 behaved as expected** (positive + `ui/` negative) |
| Benchmarks | `REPS=3 python3 benchmarks/bench.py` | see [Appendix A](#appendix-a--raw-measurements) |
| Source size | `wc -l` over `crates/*/src` | **~66k LOC of Rust** across 16 crates |
| Test density | `grep -rn '#\[test\]' crates` | **919** test functions |
| Debt markers | `grep -rn 'TODO\|FIXME\|HACK'` | **0** genuine markers in `crates/*/src` |
| Release history | `git tag` | **1 tag** (`v26.1`, 2026-07-30), 131 commits |
| Memory behaviour | peak RSS over 3M closure allocations | **7.6 MB, level with the control** (was 2,468.7 MB) — see [Appendix A](#appendix-a--raw-measurements) |

The `.sau` fixture suite is worth calling out specifically: `tests/*.sau` must
run and exit 0, and `tests/ui/*.sau` must *fail*, each one pinning a specific
diagnostic. That is a real conformance harness in embryo and it is the right
shape — see [§6](#6-how-to-know-it-is-stable) for how to grow it into a stability
gate.

---

## 3. How the language behaves

### 3.1 Type system

Saule is **statically typed, nominally class-oriented, with a Lua runtime
model**. The typing rules are stricter than Lua and less expressive than
TypeScript, which is a defensible middle.

What it has:

- **8 primitives**, with `integer` and `float` split and *never* implicitly
  promoted. `7 / 2` is `3`; mixing the two is a compile error. This kills the
  entire class of silent-truncation bugs that Lua 5.3's number model created.
- **Function types are signatures**, not a `function` tag —
  `fn(string) -> nil`. A lambda assigned into a typed slot is checked against
  arity, parameter types, and return type, and its parameters are inferred from
  the slot. The README documents the removal of the old bare `function`
  annotation and the reasoning; that was the correct call.
- **Nullability in the type** (`string?`) with `?.`, `??`, `!`, and flow
  narrowing that treats `if x != nil then` as proof inside the branch.
- **`any` is a one-way door**: anything goes in, a checked `as` cast is required
  to get out. Compare Lua's implicit dynamism or TypeScript's `any`, which
  silently infects everything downstream.
- **Generics** on functions, classes, and interfaces, with implicit
  instantiation at call sites.
- **Interfaces**, including composition, and a set of prelude interfaces the
  language itself dispatches through: `Iterable<T>`, `Iterable2<K,V>`, and the
  `Op*` family for operator overloading (`OpAdd`, `OpConcat`, `OpCompare`,
  `OpToString`, …).
- **Enums with payloads**, and `match` with guards, literal patterns, binding
  patterns, tuple patterns, and **exhaustiveness checking**.
- **Invariant table element types**, which is the sound choice and the one most
  languages get wrong on the first attempt.

Where it is incomplete or unsound:

- **The checker is deliberately partial.** `crates/saule-typeck/src/lib.rs`
  states it plainly: *"when the checker can't prove a type … it returns `None`
  and conservatively skips the check rather than producing a false positive."*
  That is the right *default* for a young language — false positives drive users
  away faster than escaped errors — but it means the static types are a strong
  lint, not a guarantee. `RuntimeError::TypeError` exists and can fire. Before
  1.0 you need to know, and document, exactly which constructs fall through.
  Today nobody can enumerate them.
- **`userdata` and `thread` are reserved and unimplemented.** The checker accepts
  `local h: userdata` and nothing in the language can produce a value for it.
  The README is honest about this, which is good; it is still a hole in the type
  grid, and `userdata` in particular is load-bearing for native packages
  ([§8](#8-is-it-easy-to-write-a-native-library)).
- **No union types, no type aliases, no structural types.** `integer | float`
  appears in the stdlib documentation (`int(n: integer | float)`) as a signature
  the *user* cannot write. That asymmetry will be noticed.
- **No `const`/immutability**, no visibility beyond class access modifiers and
  module `export`.

### 3.2 Memory model

**Reference counting, no cycle collector.** Every heap value is an `Rc`
(`Value::Table(Rc<RefCell<TableObject>>)`, `Instance`, `Function`, `Class`, …
— [value/mod.rs](crates/saule-interpreter/src/value/mod.rs)). The only `Weak` in
the entire interpreter is `FunctionObject::owner_class`.

The consequence is unavoidable and needs to be stated in the language
documentation rather than discovered:

```saule
class Node
  next: Node?
end
local a = Node()
local b = Node()
a.next = b
b.next = a     -- these two are now permanently leaked
```

Doubly-linked lists, parent↔child trees, observer registrations, and any graph
with a back-edge leak for the process lifetime. Lua does not have this problem
because it has a tracing GC; a user coming from Lua will assume Saule does too.

**The data-structure case above was the mild one.** The load-bearing cycle was
in closures, and it fired on code nobody would look at twice:

```saule
local big: table<integer> = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10}
local f: fn() -> integer = fn()
  return big[1]
end                     -- this scope could never be freed
```

A `FunctionObject` captured its defining scope by strong reference
(`closure: Rc<RefCell<Environment>>`), and `Environment` holds its bindings as
`Value`s. So the moment a function value was stored into the same scope that
created it, the graph closed:

```
Environment ──vars──▶ Value::Function ──closure──▶ Environment
```

Nothing in that cycle was weak. **Storing a lambda in a local is the single most
common way to write a helper function, and it leaked its entire enclosing scope
every time it executed** — per-execution, so a loop or a hot function
accumulated it without bound.

> **Fixed.** A lambda now captures the *names its body mentions* rather than the
> frame it was written in. The captured bindings move behind shared cells into
> one flat scope parented straight to the module root
> ([`Environment::capture_flat`](crates/saule-interpreter/src/env.rs)), so
> nothing the closure holds points back at the scope that created it and the
> cycle cannot form. Live-binding semantics are preserved: the closure and the
> original scope still read and write one location, which is what keeps
> `counter()`-style closures and per-iteration loop capture working. The capture
> set is computed once per lambda body and cached
> ([capture.rs](crates/saule-interpreter/src/capture.rs)).
>
> Measured on this machine (`target/release/saule.exe`, Windows, peak working
> set sampled during the run; all loops allocate the same 10-element table per
> iteration and differ only in what they do with a closure):
>
> | Variant | Iterations | Before | After |
> |---|---:|---:|---:|
> | No closure at all (control) | 3,000,000 | 7.5 MB | **7.5 MB** |
> | Lambda passed as an argument, never stored back | 3,000,000 | 7.6 MB | **7.6 MB** |
> | Lambda stored into the scope it captured | 3,000,000 | **2,468.7 MB** | **7.6 MB** |
> | The `Node` back-edge example above | 1,000,000 pairs | 1,282.8 MB | **1,282.8 MB** |
>
> The closure case now sits exactly on the control. The `Node` row is unchanged
> and is *supposed* to be: that is a user-authored back-edge, which is what the
> rest of this section is about.
>
> Cost: roughly 1–3% across the benchmark suite, up to ~6% on closure-heavy
> code, against a measured run-to-run noise floor of 2–3%. An earlier version
> that wrapped every binding in a `Direct | Cell` enum cost 3–8%; keeping
> captured bindings in a separate, almost-always-empty map leaves the hot
> lookup path byte-identical and gave most of that back.

Two things already narrowed the blast radius, and both are worth stating because
they also bound what is left:

- **Named `fn` declarations are top-level only** — the resolver rejects a nested
  one (*"undefined name `helper`"*), so the identical store-back in
  [stmt/mod.rs:328](crates/saule-interpreter/src/eval/stmt/mod.rs:328) only ever
  targets a module scope, which lives for the process anyway.
- **Self-recursive local closures do not capture their own name.** A lambda
  bound by `local fact = fn(n) … fact(n-1) … end` reaches itself through a
  binding the *call scope* makes to the function
  (`FunctionObject::self_name`), not through a captured cell — capturing it
  would close a fresh cycle, cell → function → captured scope → cell. Measured:
  3,355 MB over 3M iterations with the capture in place, **7.7 MB** with the
  call-scope binding.

  This shape used to be rejected outright (a `local` was not in scope inside
  its own initializer), and an earlier draft of this document treated that as
  a useful accident. It was not: the same cycle was already reachable by
  writing the recursion through any other in-scope local, e.g. a one-element
  table, which compiled and leaked. The rule bought nothing and cost a
  language feature.

What remains is the user-authored case, and it is still a real fork in the road:

- ~~**Fix the closure edge specifically.**~~ **Done** — see above. Captures now
  resolve to a flat set of shared bindings at definition time instead of
  retaining the scope chain. This is also the half that pays off in an eventual
  bytecode VM, since upvalue resolution is something that design needs anyway.
- **Accept the rest, document it, provide the tools.** Add a `weak` reference
  type to the language, document the leak, and ship a way to observe it (a
  `--stats` flag reporting live `Rc` counts at exit). This is the Swift/ObjC
  answer. Cheapest, and honest. **Note that `weak` addresses only the
  user-visible data-structure case** — it cannot reach the closure cycle, which
  is internal to the interpreter's scope representation and not something a user
  can annotate their way out of. Shipping `weak` alone would leave the larger
  leak in place while appearing to solve it.
- **Add a cycle collector.** A trial-deletion collector over `Rc` (the
  Bacon–Rajan algorithm) is the standard retrofit and does not require rewriting
  every value. Substantial work, contained blast radius. This is the only option
  that covers both cases without asking users to think about it.
- **Move to a tracing GC.** Correct in the long run, and effectively a rewrite of
  the value layer plus every native boundary.

There is also an environment recycling pool ([env.rs:44](crates/saule-interpreter/src/env.rs:44),
[recycle.rs](crates/saule-interpreter/src/recycle.rs)) — a nice optimisation, and
independent of the above.

Whichever option is taken, the §6.2 item 7 memory-behaviour test should land with
it: this class of bug was invisible for the life of the project precisely because
nothing measures it.

### 3.3 Execution model

**Two engines running one language.** A register-based bytecode VM
(`saule-vm`) is the default; the original tree-walking interpreter
(`saule-interpreter`) is still in-tree, still complete, and selected with
`saule run --interp` or `SAULE_ENGINE=interp`. Neither is a JIT.

The two are held to identical observable behaviour — output, exit status and
error text — by a differential harness that runs every `.sau` fixture and every
example project under both, plus 209 differential tests in Rust. That harness
is the reason the tree-walker is kept rather than deleted: it is the oracle,
and it has caught every VM bug found so far.

The bytecode compiler does not yet reach the whole language. A module it
cannot compile **falls back to the tree-walker** rather than guessing, so a
gap costs speed and never correctness. Today **9 of the 11 example projects
run entirely on the VM** and `run_examples_diff.sh` reports **0 fallbacks**;
the two that remain are refused by design, on `an import of a dynamic native
package`, since loading one is a runtime side effect that compiling must not
perform. At the file level, 87 of 92 `tests/*.sau` fixtures compile fully.

**A note on how that is measured, because this section previously reported it
wrongly.** It once read "the remaining refusals are concentrated in
cross-module class hierarchies", a conclusion drawn from running `saule
disasm` over individual `examples/**/*.sau` files. That census compiles one
file through the *single-module* path, while a real program compiles through
`program::compile`, which walks the import graph — so files that refuse
standalone routinely compile fine as part of their project. Steer by the
per-project number above, not by a per-file one.

Consequences:

- **Performance is now what a bytecode VM gives you.** Measured three times,
  on two machines, and they agree on shape rather than on digits — quote a
  range, and quote which row. The first two rows are the *same* box and the
  same Lua, before and after Phase 5's first slice, so they are the only pair
  here that may be read as a change rather than as a difference of machine.

  | | vs PUC Lua | VM over tree-walker |
  |---|---|---|
  | Windows x86_64, Lua 5.4.8, after Phase 5's peepholes | 0.8×–4.8× (walker 0.9×–9.4×) | 1.24×–3.52× |
  | Windows x86_64, Lua 5.4.8, at the Phase 4 flip | 1.0×–4.8× (walker 5.5×–9.0×) | 1.15×–2.80× |
  | macOS arm64, Lua 5.5.0 | 1.1×–4.5× (walker 1.3×–12.8×) | 1.20×–3.32×, geomean 2.19× |

  The exceptions are the same on both: `map` and `sort` barely move (1.20×,
  1.28× on macOS), because their time goes on hashing and comparator
  callbacks inside the table implementation rather than on dispatch. On
  macOS, LuaJIT 2.1 is 3.5×–40.5× ahead of the VM — a tracing JIT against an
  interpreter, so a categorical gap rather than a to-do list. `startup` is
  faster than Lua on Windows and slower on macOS; it measures process launch,
  not the front end. Full tables in
  [Appendix A](#appendix-a--raw-measurements).
- **Native stack depth is the recursion limit.** `MAX_EVAL_DEPTH = 10_000`
  ([eval/mod.rs:35](crates/saule-interpreter/src/eval/mod.rs:35)) converts what
  would be a SIGSEGV into a catchable `RuntimeError::StackOverflow`. Commit
  `60ccc75` shows the depth of thought here — the main thread gets a 512 MiB
  stack via a linker flag on macOS because AppKit forbids running the interpreter
  on a spawned thread. This is careful work.
- **Tail calls are eliminated in both engines** — a trampoline in the
  tree-walker, `TAILCALL`/`TAILCALLK`/`TAILCALLS` in the VM — so `return f(...)`
  in tail position runs in constant frame depth. `down(500_000)` returns
  under both. They had to land together: whichever engine got it first
  would have disagreed with the other about which programs overflow.

This was the multi-month project — the original draft of this document called
a bytecode VM "the standard next step" and warned it would invalidate parts of
the LSP's story. It did not: `saule-lsp` and `saule-db` execute no user code,
so neither links `saule-vm` and neither needed a line changed. What is still
true is the ranking — the remaining performance work is optimisation with a
profile in hand (inline caches, superinstructions), and it is worth less than
any single item in [§5](#5-the-gaps-ranked).

### 3.4 Error model

Compile-time diagnostics are genuinely excellent. `miette` renders every phase
with source snippets, labels, and `help` text; imported modules carry their own
`NamedSource` so a failure in a dependency underlines the right file
([error.rs:100](crates/saule-interpreter/src/error.rs:100)). `saule check`
reports every diagnostic rather than stopping at the first, and exits non-zero
so it can gate CI. The `tests/ui/` fixtures pin the diagnostics.

Runtime errors are where it thins out:

- **There is no stack trace.** Grepping the interpreter for `backtrace`,
  `stack_trace`, or `call_stack` returns nothing. A runtime error reports the
  span where it fired and nothing about how execution got there. In a language
  with classes, closures, and interfaces, "attempt to index a nil value at
  Foo.sau:41" without the calling frames is a bad debugging experience, and it is
  the single most-felt gap after the parser.
- **`throw` is stringly-typed at the boundary.** `RuntimeError` must be
  `Send + Sync` for miette, so the thrown `Value` is parked in a thread-local and
  the error carries only its `Display` form. It works, but it means the error
  path has a side channel — a source of subtle bugs if anything ever re-enters.
- **The stdlib's filesystem errors are booleans.** `Os.mkdir`, `Os.remove`,
  `Os.rename`, `Os.chdir` return `boolean` with no error detail, which DOCS.md
  admits ("no detailed error type yet"). Users cannot distinguish
  permission-denied from not-found. This wants an error enum before people write
  real tooling in Saule.

**One error class per phase, and one runtime error type** is the right design; the
gap is depth of information, not structure.

### 3.5 Module and project model

`saule.config` is a flat `key: "value"` file with `--` comments — deliberately
not TOML/YAML, and small enough that both the CLI and the LSP hand-roll a parser
for it. Imports are path-based, resolved by the CLI (not the interpreter), and a
dependency's import name comes from *its own* config rather than its path. That
last decision is the reason RELEASE_PLAN.md can propose a package manager with
no central index, and it is a good one.

Behaviour worth knowing:

- **Two run modes, decided by one thing**: directory ⇒ project mode (requires
  `class Main` with `static fn main()`); file ⇒ script mode (runs top to bottom).
- **Folder modules** via `init.sau` barrels, with a `MAX_BARREL_DEPTH = 8` bound
  that doubles as the cycle guard.
- **Module loading is cached per `ModuleLoader`**, shared through the environment
  so transitive imports hit the same cache.
- **Cross-module typechecking works through an "import seed"** — the checker
  walks the import graph, reads and parses every reachable module, and seeds the
  registries. This was correct but redundant: the walk starts afresh from every
  file, so a project's shared modules were re-parsed once per importer. Both the
  CLI and the LSP now drive it through [saule-db](crates/saule-db/src/lib.rs),
  which memoises the walk and hands it trees it has already parsed —
  `saule check` on `examples/UI Project` went from 0.24 s to 0.07 s. See
  [§9.2](#92-new-crates-to-extract), including a correction to the ~27 ms figure
  this document previously quoted.
- ~~**`saule.config` is parsed twice**~~ — **fixed.** The format, project
  discovery, dependency resolution and source scanning now live in
  [saule-project](crates/saule-project/src/lib.rs), which `saule-cli`,
  `saule-lsp` and `saule-interpreter` all read. The two parsers had already
  drifted: the LSP's understood four of the seven keys, and the CLI's file
  walker knew about only one of the two source extensions, so `saule check`
  silently skipped every `.saule` file. Both are gone with the duplication.

### 3.6 Concurrency model

There is none. No coroutines (`thread` is a reserved, unimplemented type name),
no threads, no async, no channels. The interpreter is `Rc`-based and explicitly
thread-confined; every registry is `thread_local!`.

For a Lua-lineage language this is a notable absence — coroutines are one of
Lua's defining features and the idiom people bring with them. This section used
to argue that implementing them on a recursive `eval` needed OS threads,
stack copying, or a bytecode VM with an explicit frame stack, and that
**coroutines were effectively a vote for a bytecode VM**. That VM now exists
and is the default (§3.3), and its frame stack is an ordinary `Vec` of frames
in one `Vm` — the shape a coroutine implementation wants. The obstacle is no
longer the engine; it is that a coroutine has to mean something under *both*
engines, or the differential harness that keeps them honest stops applying.

---

## 4. Scorecard against a production language

Weighted by what actually stops adoption, not by what is hard to build.

| Dimension | State | Grade |
|---|---|---|
| Language design coherence | Opinionated, consistent, well-reasoned | **A** |
| Implementation quality | Clean, tested, documented, lint-gated | **A** |
| Compile-time diagnostics | miette, cross-module, `ui/` pinned | **A−** |
| Documentation (language) | README 73k + DOCS 15k, examples, website | **A−** |
| Type system completeness | Strong core, deliberately partial checker, no unions/aliases | **B** |
| Formatter | Dedicated crate, config-driven, corpus tests, round-trips | **A−** |
| LSP feature breadth | Hover, goto, refs, symbols, inlay, sighelp, completion, format | **A−** |
| LSP robustness | Error recovery + prior-parse seeding + completion repair | **B+** |
| Runtime performance | Bytecode VM by default: 0.8×–4.8× PUC Lua, 1.24×–3.52× the tree-walker. Phase 5's peepholes cut instructions retired 11–50% — `loop_arith` halved | **B** |
| Memory management | Refcount; closure capture fixed; user-authored cycles still leak, no tooling to see them | **C** |
| Runtime diagnostics | No stack traces, boolean FS errors | **C−** |
| Concurrency | Absent | **D** |
| Standard library | Solid core; no regex, JSON, net, structured errors | **C** |
| Test tooling for users | None | **F** |
| Debugger | None (no DAP) | **F** |
| Distribution / install | 1 tag, no published artifact, build-from-source | **D** |
| Package management | Designed in detail, not built | **F** |
| Embedding story | Works, undocumented, no facade crate, global state | **C−** |
| Native library authoring | Excellent design, no distribution, no ABI check | **C+** |
| Stability policy | None | **F** |
| Governance / contribution | LICENSE + CI; no CONTRIBUTING, no RFC path, no CoC | **D** |

The pattern is unmistakable: **everything that is code is good, and everything
that is contract, distribution, or user-facing tooling is absent.** That is
normal for a solo-built language and it is a very fixable position — the hard
part (a coherent, working implementation) is done.

---

## 5. The gaps, ranked

### Tier 0 — blocks *any* outside user

1. **No installable toolchain.** `scripts/install_path.sh` symlinks a build from
   a clone. One tag exists; the release workflow builds six triples but has
   effectively never published. RELEASE_PLAN steps 2–3 cover this correctly.
   *Until this ships, the user count is 1.*
2. **No package manager.** `dependencies:` takes local paths. RELEASE_PLAN steps
   5–8 cover this and the design (no index, git-hosted, exact pins, SHA lock) is
   sound.
3. ~~**Parser gives up on the first error.**~~ **Done** — see the note in
   [§1](#1-the-verdict-in-one-page). `parse_recover` always produces a tree, and
   the LSP builds on it. The follow-on question is no longer *whether* the editor
   survives a syntax error but how good the recovered tree is; the 33 recovery
   tests are the place to grow that, and §6.2's diagnostic snapshots would make
   regressions in it reviewable.
4. **Editor plugins are unpublished.** All three exist and work; none are on a
   marketplace. A language you must sideload editor support for is a language
   people bounce off.

### Tier 1 — blocks serious use

5. **No test runner.** There is no `saule test`, no assertion library beyond the
   prelude `assert`, no test discovery, no reporting. Nobody writes production
   code in a language they cannot test. This is cheap to build relative to its
   impact.
6. **No stack traces on runtime errors.** See §3.4.
7. **User-authored reference cycles leak.** See §3.2. The closure case — which
   was the severe one, firing on ordinary `local f = fn() … end` and costing
   2.5 GB against a 7.5 MB control — is fixed. What is left is the case a
   programmer can see and reason about: a back-edge between values, like the
   `Node` pair. It still leaks for the process lifetime, there is still no way
   to observe it, and `weak` plus a cycle report is still the answer.
8. **No debugger.** No DAP implementation. Print debugging only.
9. **Performance.** The bytecode VM landed and is the default, which moved this
   from 5–11× PUC Lua to 0.8×–4.8× (§3.3). It is no longer the thing that
   decides whether Saule is usable — every item above it on this list matters
   more.

   The caveat this entry used to carry — "some real projects still take the
   tree-walking fallback and get the old numbers" — has largely been paid off:
   **9 of the 11 example projects now run entirely on the VM**, and
   `run_examples_diff.sh` reports **0 fallbacks**. The two that remain are
   refused by *design*, on `an import of a dynamic native package`, because
   loading one is a runtime side effect that compiling must not perform. Three
   single-file gaps are left (an enum with methods, a prelude name outside a
   call, `self` outside a method); each costs speed on the file that uses it,
   never correctness.
10. **Stdlib holes that force a native package**: pattern matching / regex
    (Lua's `string.match`, `gmatch`, `gsub` have no equivalent — `String.find`
    is literal-only), JSON, structured filesystem errors, date/time beyond
    `Os.date`, and any networking.

### Tier 2 — blocks the ecosystem

11. **SDK crates unpublished on crates.io.** See [§8](#8-is-it-easy-to-write-a-native-library).
12. **No ABI version check when loading a native package.** The interpreter
    `dlopen`s the library and transmutes symbols to `NativeSymbolFn` with the
    comment *"a mismatch is the package author's bug"*
    ([bind.rs:266](crates/saule-interpreter/src/dynamic_packages/bind.rs:266)).
    A `.dylib` built against an older `CValue` layout is undefined behaviour, not
    an error message. With a package manager shipping prebuilt binaries, this
    stops being theoretical.
13. **No documentation generator.** `saule-docs` extracts `---` doc comments and
    is consumed only by the LSP. There is no `saule doc` producing HTML for a
    library's users, which is table stakes for a package ecosystem.
14. **No language specification separate from the tutorial.** README.md is a very
    good tutorial with a grammar appendix. A spec is a different document: it
    says what is *guaranteed*, and it is what an alternate implementation, a
    conformance suite, and a stability policy are all written against.
15. **No contribution path.** No `CONTRIBUTING.md`, no issue templates, no RFC
    process, no code of conduct. A consumer language needs contributors, and
    contributors need a door.

### Tier 3 — worth knowing about

16. **`tower-lsp 0.20`** — verify its maintenance status before 1.0; the
    ecosystem has been consolidating on a maintained fork
    (`tower-lsp-server`). A dead LSP framework is a slow-motion problem.
17. **CI tests one platform.** `ci.yml` runs on `ubuntu-24.04` only, deliberately
    ("the release workflow builds all six triples"). But the release workflow
    *builds*, it does not *test*. Nothing has ever run the test suite on Windows
    or macOS. Given filesystem, path-separator, and DPI code that is explicitly
    platform-conditional, that is a real risk.
18. **`saule-engine-lib` is excluded from clippy and from release builds.** It is
    6.5k lines of unlinted code sitting in the toolchain workspace.
19. **The LSP re-derives types.** *(Unchanged by the query-layer pass, and now
    with a named blocker — see [§9.2.1](#921-what-blocks-the-rest-of-the-query-layer).)*
    `exprty.rs` was written specifically to
    consolidate two drifting copies of expression typing, and its module doc lists
    four concrete bugs the drift caused (`??` nullability, `or` result type,
    unary minus, operator overloads). It fixed the symptom; the cause is that the
    LSP cannot ask the checker "what is the type here?" and must re-implement it.
    ~800 lines of walker inference still mirror `saule-typeck`.

---

## 6. How to know it is stable

"Stable" is not a feeling and not a version number. It is **a written contract
plus mechanised evidence that the contract holds.** Saule currently has neither,
and that is the gap this section closes.

### 6.1 Write the contract first

Publish a `STABILITY.md` that answers, for each surface, *what may change*:

| Surface | Proposed contract |
|---|---|
| Language syntax and semantics | Additive only within a year. A program valid under `26.x` runs identically under every later `26.y`. Removals require a deprecation cycle spanning a year boundary. |
| Standard library | Additive only. Signatures never narrow. Behaviour changes only to fix a documented bug, and the fix ships with a `ui/` fixture. |
| Diagnostic **codes** | Stable and never reused. Diagnostic *text* is explicitly unstable — say so, so nobody parses it. |
| `saule.config` keys | Additive. Unknown keys already ignored (good); that becomes a promise. |
| CLI surface | Subcommands and flags additive; output format of `--json` modes (once they exist) versioned. |
| Native ABI (`CValue`, `HostApi`) | **Frozen**, versioned separately from the toolchain, with a runtime check. Any change is a new ABI major and the interpreter refuses the old one *by name*. |
| Rust crate APIs (`saule-interpreter`, …) | Explicitly **unstable** unless and until you publish a `saule` facade crate. Say this loudly or embedders will assume otherwise. |
| Wire protocol of the LSP | Follows the LSP spec; no Saule-specific extensions without a version. |

The year-based `26.<build>` scheme is a good fit for this, and RELEASE_PLAN
already justifies it well. Its one weakness is that it carries **no signal about
breakage** — `26.7` → `26.8` could be anything. The contract above supplies the
missing signal: *within a year, additive only.* That makes the version scheme
mean something without adding a patch component.

### 6.2 Then mechanise it

A contract nobody tests is a wish. Each of these is a gate that fails a PR:

1. **Conformance suite, separated from the test suite.** `tests/*.sau` today is a
   regression suite — it tests the implementation. A conformance suite tests the
   *specification*: one directory, one file per spec section, every file
   cross-referenced to the spec paragraph it pins. Growing `tests/ui/` into
   "every diagnostic has exactly one fixture that produces it" is the same idea
   for errors. *Signal: coverage of the spec, measured and reported.*
2. **Backwards-compatibility CI.** Keep a corpus of programs — the examples, the
   benchmarks, `www`'s samples, and every real project you have — and run the
   *current* build against *every previously released* corpus on each PR. The
   moment a `26.7` program breaks on `26.9`, CI says so. This is the single most
   valuable stability gate and it is cheap to build.
3. **Snapshot-test the diagnostics.** `insta` or equivalent over the `ui/`
   fixtures, so a change in wording is a reviewable diff rather than an invisible
   change. Today `run_tests.sh` only checks that a `ui/` fixture *fails*, not
   *how* — so a diagnostic could silently regress into a worse one and CI stays
   green.
4. **Fuzz the front end.** `cargo-fuzz` on lexer → parser → semantic → typeck,
   asserting no panics and no hangs. A production compiler must not crash on any
   input, and 60 `unwrap`/`expect` sites in the parser is exactly where a fuzzer
   earns its keep. **Zero panics on arbitrary input** is a crisp, checkable
   stability criterion.
5. **Benchmark regression gate.** `bench.py` already supports `new=` / `old=`.
   Wire it to CI with a threshold so a change cannot silently cost 20%.
6. **Multi-platform test matrix.** Run the *test suite* — not just the build — on
   Linux, macOS, and Windows. See §5 item 17.
7. **Memory-behaviour tests.** Assert that a program's live-value count returns
   to baseline after a workload. This is how the cycle-leak class of bug becomes
   visible instead of anecdotal.
8. **ABI compatibility test.** Build a native package against the published SDK
   version, load it with the current interpreter, assert it works — or is
   rejected with a clear message.

### 6.3 The criteria for declaring 1.0

Concretely, Saule is ready to call itself stable when *all* of these hold:

- [ ] `STABILITY.md` exists and every gate in §6.2 is live in CI.
- [ ] The spec is written, separate from the tutorial, and the conformance suite
      cites it section by section.
- [ ] Fuzzing has run for a sustained period with zero panics outstanding.
- [ ] The backwards-compatibility corpus spans at least a year of releases with
      zero unintentional breaks.
- [ ] Test suite green on all three desktop platforms.
- [ ] The native ABI is versioned and checked at load time.
- [ ] A non-author has shipped something real with it and reported back.

That last one is not a formality. Every list above is written from the inside;
the first outside user finds a different list within a day.

---

## 7. Is it easy to embed?

**It works, it is undocumented, and it has sharp edges the embedder will find at
the worst moment.**

### What is genuinely good

The architecture anticipated embedding, and in a couple of places did it
properly:

- **The pipeline is a library, not a binary.** `saule_interpreter::check_and_run`
  runs semantic → typeck → eval and is explicitly labelled *"the entry point most
  embedders should use"* ([lib.rs:176](crates/saule-interpreter/src/lib.rs:176)).
- **Output is redirectable.** `output::Sink` /
  `output::capture(|| …)` lets the host route `print` anywhere, and the doctest
  in [output.rs:16](crates/saule-interpreter/src/output.rs:16) shows it in three
  lines. This exists because the wasm playground needed it, and it is exactly the
  right abstraction.
- **Host facilities are pluggable.** The `Platform` trait
  ([platform.rs](crates/saule-interpreter/src/platform.rs)) abstracts clock,
  sleep, pid, and exit, with every method defaulting to "unavailable", so a
  sandbox implements only what it can do. `Os.time()` in a browser returns a
  clean error instead of trapping the module. This is well-judged design.
- **wasm is a first-class target.** `saule-interpreter` compiles for
  `wasm32-unknown-unknown` with `default-features = false`, and `saule-wasm`
  proves the whole embedding path works. **This is the strongest evidence the
  embedding story is real** — someone already did it end to end.
- **Callable from the host.** `call_function_value` and
  `call_class_static_method` let the host invoke Saule functions without
  re-parsing.
- **Sandboxing exists.** `tests/sandboxed_platform.rs` covers it.

### The friction

1. **There is no facade crate.** An embedder must depend on `saule-lexer`,
   `saule-parser`, `saule-semantic`, `saule-typeck`, and `saule-interpreter`
   separately, and know the order to call them in. There is no `saule` crate that
   re-exports one clean API. Every embedder will get this subtly wrong.
2. **Nothing is on crates.io.** Embedding means a git dependency on the whole
   workspace.
3. **All state is process- or thread-global.** 22 `thread_local!` blocks across
   the analysis crates, plus process-wide `RwLock` statics for native package
   registries. There is no `Interpreter` handle. Consequences an embedder hits:
   - **You cannot run two isolated Saule instances on one thread.** Two scripts
     with different sandbox policies, different sinks, or different registered
     natives cannot coexist.
   - The `init()` `Once` is process-wide, so package registration is
     first-caller-wins.
   - Everything is thread-confined by `Rc`, which is fine and documented — but
     it means a server embedding Saule needs one thread per instance, and each
     thread re-pays the registry setup.
   A `Interpreter::new()` handle owning what is currently thread-local would fix
   this. It is a large refactor and should be decided *before* the API is
   published, because it changes every signature.
4. **No embedding documentation.** No `EMBEDDING.md`, no example, no doctest
   beyond `output`. The wasm crate is the de facto example and nothing points at
   it.
5. **No API stability statement**, so an embedder cannot know what will break.
6. **Registering host functions is undocumented.** `native_packages::register`
   exists and the stdlib uses it, but the path for an embedder to expose their
   own Rust functions to a script — the single most common embedding need — is
   nowhere described.

### Verdict

For a motivated Rust developer reading the source: **yes, and pleasantly so.**
For someone who wants to add scripting to their app in an afternoon: **no**, and
the blockers are documentation and packaging, not architecture. Publishing a
`saule` facade crate with an `EMBEDDING.md` and three examples (run a script,
capture output, register a host function) would move this from C− to B+ without
touching the interpreter.

---

## 8. Is it easy to write a native library?

**The authoring experience is excellent. The distribution experience is the worst
part of the project.**

### What is genuinely good — and this is the best-designed subsystem in the repo

Writing an export is one attribute on a safe Rust function:

```rust
#[saule_export(class = "Graphics", name = "circle")]
fn graphics_circle(mode: String, x: f64, y: f64, r: f64) -> Result<(), String> {
    // no unsafe, no CValue, no manifest entry
}
```

`#[saule_export]` generates the `extern "C"` shim, the arity check, argument
decoding, and error marshalling — and, critically, **infers the Saule signature
from the Rust types**, so the manifest cannot drift from the code. The manifest
is *generated* by a `gen-manifest` binary walking the `inventory` registrations,
not hand-maintained. An `Err(e)` becomes a Saule runtime error at the call site.
Tuple returns become Saule multi-returns.

The ABI itself ([saule-native-abi/src/lib.rs](crates/saule-native-abi/src/lib.rs))
is 314 lines, `#[repr(C)]`, and documents its string-ownership contract precisely
— arguments borrowed for the call, returns living in a thread-local until the
next call, host copies immediately. Reference values (`table`, `fn`) never cross
by memory; the package gets an opaque `Handle` and manipulates them through a
`HostApi` vtable, with handle lifetime bounded to the call. `SFunction`
parameters must declare their Saule signature in the attribute, and *omitting it
is a compile error* rather than a silently untyped parameter. That is the kind of
detail that separates a designed ABI from an evolved one.

And the payoff is real: **manifests are parsed at startup, so a native package's
methods type-check and appear in LSP completion before the shared library is ever
loaded.** Very few scripting languages do this.

### The problems

1. **The SDK crates are not published.** `saule-sdk`, `saule-native-abi`, and
   `saule-export-macro` are path dependencies inside this workspace. To write a
   native package today you must clone the Saule compiler and add your crate to
   its workspace. **This is a hard stop on third-party native packages**, and it
   is the cheapest high-value fix available: publish three small crates.
2. **Installation is manual file copying.** From the example README: build, `mkdir`
   two directories, copy the `.so`/`.dll`/`.dylib` (renaming to strip the `lib`
   prefix on Unix, because the manifest lists an unprefixed name), then run
   `gen-manifest` with an output path. Four steps, platform-specific, with a
   naming trap. Users will get this wrong. RELEASE_PLAN Appendix F fixes it via
   `saule add` pulling GitHub release assets — that plan is right and should
   land.
3. **No ABI version check.** Covered in §5 item 12. The manifest has `name`,
   `version`, and `binary`, but no `abi_version`, and nothing is verified at load.
   Add `abi_version` to the manifest **and** a required `saule_abi_version`
   symbol, and refuse to load on mismatch with a message naming both versions.
   Do this before the package manager ships prebuilt binaries.
4. **No opaque handle type.** The ABI's tags are nil/bool/int/float/str/err/table/func.
   A package that owns a resource — a texture, a socket, a database connection —
   must hand Saule an `i64` index into its own registry. The engine does exactly
   this: `Graphics.newImage(path) -> i64`
   ([graphics.rs:485](crates/saule-engine-lib/src/graphics.rs:485)). It works, but
   it means no type safety (every handle is `integer`, interchangeable with any
   other), and **no destructor** — nothing tells the package when Saule drops the
   last reference, so resources live until the package clears its registry. This
   is precisely what `userdata` is reserved for (§3.1), and it is the main reason
   to implement it.
5. **No cross-compilation help.** A package author must produce six binaries. A
   reusable GitHub Actions workflow shipped as a template would remove most of
   that work.
6. **`saule-engine-lib` is a poor exemplar.** It is the reference example for
   native packages, and it is 6.5k lines of graphics engine, excluded from clippy,
   living inside the compiler workspace. An author looking for "how do I write a
   package" has to filter a rasterizer, a font engine, and a PNG decoder out of
   the answer. A 200-line example package in its own repository would teach far
   more.

### Verdict

The design is a genuine strength — better than Lua's raw C API and comparable to
the ergonomics of `pyo3` or `napi-rs` for the subset it covers. Everything
blocking it is packaging. **Publish the three crates, add an ABI version check,
add `userdata`, and ship a minimal example repo**, and this becomes the feature
people mention when they recommend Saule.

---

## 9. Crate and repository topology

### 9.1 The current 16 crates

| Crate | LOC | Role | Assessment |
|---|---:|---|---|
| `saule-ast` | 865 | Shared AST | Correct. Foundation for everything. |
| `saule-lexer` | 1,103 | Tokeniser | Correct. |
| `saule-parser` | 2,995 | Recursive descent + error recovery | Correct. Recovery landed in `197ef4f`; LOC is now higher than shown. |
| `saule-semantic` | 2,565 | Resolution, registries, flow, field-init | Correct. |
| `saule-typeck` | 8,043 | Types, nullability, generics, exhaustiveness | Correct. |
| `saule-interpreter` | 15,046 | Tree-walker, stdlib, modules, native hosting | **Too broad — see 9.2.** |
| `saule-fmt` | 2,943 | Formatter | Correct, well-isolated. |
| `saule-docs` | 1,015 | `---` doc-comment extraction | Correct; under-used (§5.13). |
| `saule-cli` | 1,514 | `saule` binary | Correct. |
| `saule-lsp` | 20,754 | Language server | **Largest crate; see 9.3.** |
| `saule-db` | 938 | Incremental query layer | New in this pass — see 9.2. |
| `saule-native-abi` | 314 | Frozen C ABI | Correct. **Must be published.** |
| `saule-sdk` | 1,268 | Package-authoring SDK | Correct. **Must be published.** |
| `saule-export-macro` | 502 | `#[saule_export]` | Correct. **Must be published.** |
| `saule-project` | 813 | `saule.config`, discovery, deps, `ProjectInfo` | New in this pass — see 9.2. |
| `saule-version` | 417 | Build-time version resolution | Correct, clever, self-contained. |
| `saule-wasm` | 446 | Playground bindings | Correct. Candidate for its own repo. |
| `saule-engine-lib` | 6,583 | Graphics engine example | **Does not belong here — see 9.5.** |

The split is, on the whole, better than most language projects manage. The
pipeline crates are cleanly layered and the dependency graph flows one way. What
follows are the changes worth making.

### 9.2 New crates to extract

**~~`saule-project` — extract now, highest value.~~ Done.**
Owns `saule.config` parsing, project discovery, `src_dirs`/`dependencies`
resolution, source scanning, and `ProjectInfo`. This logic used to be
implemented twice, in `saule-cli/src/project.rs` and `saule-lsp/src/workspace.rs`,
the second carrying a comment explaining that duplication was chosen because the
CLI's parser is private and depending on the CLI would invert the dependency
direction. That reasoning was right and the conclusion was wrong; the answer was
a third crate both depend on, and it is now
[crates/saule-project](crates/saule-project/src/lib.rs). `saule-interpreter`
re-exports it as `project`, so `saule_interpreter::project::…` still resolves.

Two config parsers that must agree and are separately maintained is a bug
waiting for its moment, and the wait was over before the extraction started —
see [§3.5](#35-module-and-project-model) for the two divergences found on the
way out.

One claim in the original draft was wrong and is worth correcting rather than
quietly deleting: extracting this crate does **not** remove `saule-lsp`'s
dependency on `saule-interpreter`. The server reaches into the interpreter for
`module::collect_import_seed`, `module::resolve_import_path`,
`native_packages`, `dynamic_packages`, `stdlib::all_prelude_names` and `init()`.
`ProjectInfo` was the smallest of those edges, not the load-bearing one. What
severs the rest is the query layer below.

**`saule` — the facade, extract before publishing anything.**
One crate an embedder depends on. Re-exports `Value`, `RuntimeError`,
`PipelineError`, the `Platform` and `Sink` traits, and a single documented entry
point. Carries `EMBEDDING.md`, the doctests, and — crucially — **the API
stability guarantee**, so the underlying crates stay free to change. Without
this, publishing to crates.io freezes five internal APIs by accident.

**`saule-stdlib` — extract when the stdlib grows.**
The standard library is 12 modules inside `saule-interpreter/src/stdlib/`. It is
a distinct concern from evaluation, it is what will grow fastest (regex, JSON,
structured errors), and separating it makes "what does a minimal embed pull in?"
answerable. Not urgent; do it when the stdlib next doubles.

**`saule-test` — new, needed.**
The `saule test` runner (§5.5). Test discovery, assertions, reporting, exit codes.
Needs its own crate because the CLI, CI, and eventually the LSP (run-test
codelenses) all consume it.

**`saule-db` / query layer — the strategic one. Half done.**
The 27 ms-vs-0.4 ms seed measurement (§3.5) and the LSP's duplicated inference
(§5.19) are the same problem from two directions: there is no incremental,
memoised layer between "files on disk" and "answers about the code".

[crates/saule-db](crates/saule-db/src/lib.rs) is now that layer, for the front
half of the pipeline. It is a small `salsa`-shaped engine — one global revision
counter, dependency edges recorded as queries run, and **early cutoff** on
recomputed values — carrying three queries: `parsed`, `imports`, and `seed`.
The engine is ~150 lines
([engine.rs](crates/saule-db/src/engine.rs)); writing it rather than taking
`salsa` was a judgement call about a four-node graph, and is worth revisiting if
the query set grows.

Early cutoff is the whole design and it is worth being precise about why. `seed`
depends on `imports`, never on the file's text directly. Typing in a function
body invalidates the text and the parse tree, so `imports` is recomputed — and
comes out *equal*, which stops the invalidation there. The seed is not rebuilt.
That is the property the hand-rolled cache was approximating with a bespoke rule,
and it is now a consequence of the graph rather than a thing to remember.

What it bought, measured:

| | before | after |
|---|---:|---:|
| `saule check "examples/UI Project"` (34 files, warm, release) | 0.24 s | **0.07 s** |

The speedup is not from caching the seed — each file has its own — but from the
walk no longer re-parsing. `collect_import_seed` starts from every file in turn,
so the modules near the root of the import graph were lexed and parsed dozens of
times per run. The walk now accepts trees from whoever is driving it
(`module::SeedIo`), and the database hands over the ones it already has.

Two corrections to the original draft, both from measuring rather than reading:

- **The ~27 ms figure does not reproduce.** In a release `saule check`, the whole
  pipeline costs ~7 ms per file for this project, seed included. The measurement
  in [seed_cache.rs](crates/saule-lsp/src/server/seed_cache.rs)'s comment was
  never re-taken after the parser work landed, and this document repeated it. The
  *shape* of the claim held — the walk dominated, and it was duplicated work —
  but the number did not.
- **Caching the file text changed nothing at all.** The reads were already free
  against the OS page cache; the cost was entirely lexing and parsing. The text
  cache stayed because it is still the right structure, not because it showed up.

What remains is the other direction: the ~800 lines of LSP-local inference, which
needs a `check` query returning a span→type map. That one is blocked on something
this pass surfaced and nobody had written down — see
[§9.2.1](#921-what-blocks-the-rest-of-the-query-layer).

#### 9.2.1 What blocks the rest of the query layer

`saule-semantic` and `saule-typeck` communicate through **thread-local
registries**: `analyze_with_seed` installs the class / interface / enum tables
and `check` reads them. Callers depend on that side effect — completion runs
`analyze_with_seed` purely for it and throws the diagnostics away.

A memoised `analysis(file)` query cannot preserve that. Returning cached
diagnostics on a hit would skip the registry installation every downstream
caller is silently relying on, and hover would start answering against whatever
the last analysed file happened to leave behind. So the query layer stops
cleanly at the seed, and **the prerequisite for going further is making the
registries a value that a query can return** rather than a thread-local a pass
installs. That is the same refactor §7 asks for on the embedding side, where the
22 `thread_local!` blocks are what stop two interpreters coexisting on one
thread. One change, two payoffs — and it should be sequenced before either.

**`saule-syntax` — reconsider, now that recovery exists.**
The original rationale was that recovery would need an error-tolerant, lossless
CST (rowan-style), which is a different data structure from the current `Module`
and would deserve its own crate. Recovery instead landed inside `saule-parser` as
[recover.rs](crates/saule-parser/src/recover.rs), producing a `Parsed` tree with
holes rather than a full CST — a smaller change that has so far been enough. Do
not extract this crate on the old reasoning. Revisit only if a concrete need for
losslessness appears (whitespace-preserving refactors, or a formatter that wants
to round-trip broken input), and treat that as a fresh decision.

### 9.3 Crates to split

**`saule-lsp` is doing three jobs**: protocol plumbing
(`tower-lsp`, dispatch, document cache), semantic analysis for editors
(hover walkers, inference, references, symbols), and feature handlers. The
analysis half is the valuable, reusable part and is currently welded to a
specific LSP framework. If §9.2's query layer happens, most of the analysis half
moves there and this resolves itself. Otherwise split
`saule-ide` (framework-independent analysis) from `saule-lsp` (protocol).

**`saule-interpreter` (15,046 LOC)** is the other broad one: evaluator + stdlib +
module loader + native hosting + platform + project types. `saule-stdlib` and
`saule-project` (§9.2) take the two clearest slices; the rest is coherent.

### 9.4 What must be published to crates.io

In dependency order, and this is the shortest path to an ecosystem:

1. `saule-native-abi` — **frozen, versioned independently of the toolchain.**
   This crate's version *is* the ABI version and should be plain semver (`1.0.0`),
   not `26.x`. Its major number is what the load-time check compares.
2. `saule-export-macro`
3. `saule-sdk`
4. `saule` (the facade, once it exists)

A version wrinkle worth resolving now: the workspace pins `version = "26.0.0"`
with a comment that Cargo's copy is internal metadata nothing user-facing prints.
That is true today and false the moment anything is published — crates.io will
show `26.0.0` and users will read it as a year-scheme version of a library whose
ABI is nothing of the sort. **The published crates need their own semver
versions, decoupled from the toolchain's `26.<build>`.** RELEASE_PLAN already
accepts two version schemes (toolchain vs. packages); this is a third, and it is
the right one for the ABI.

`saule-cli`, `saule-lsp`, `saule-interpreter`, and the pipeline crates should be
`publish = false` until you are willing to guarantee their APIs.

### 9.5 What should be its own git repository

Split when the release cadence, the audience, or the CI needs differ. By that
test:

| Move out | Why | When |
|---|---|---|
| **`saule-engine-lib` → `saule-lang/saule-engine`** | It is an application, not toolchain. Excluded from clippy and from release builds — it is already half-out. Making it a standalone repo consuming the *published* SDK **dogfoods the entire native-package path**: if the engine can't be built and installed by an outsider, neither can anyone else's package. | With the SDK publish |
| **`editors/vscode` → own repo** | Marketplace publishing, npm toolchain, independent cadence, its own CI. | With RELEASE_PLAN step 4 |
| **`editors/intellij` → own repo** | Gradle/JVM build; nothing shared with the Rust workspace. | Same |
| **`editors/nvim` → own repo** | Plugin managers install from a repo root. | Same |
| **`www` → own repo (or keep, deliberately)** | Node toolchain, `node_modules` and `dist` inside the language repo, and two of the four CI workflows exist to serve it. Counter-argument: `check-www.yml` verifies documented samples compile against the compiler *from that commit*, which is a genuinely valuable coupling and would become a cross-repo dance. **Keep it, but move the sample-verification corpus into `tests/` so the guarantee survives a later split.** | Deliberate decision |
| **A minimal `saule-package-template` repo** | The example an author actually copies. 200 lines, one class, CI producing all six binaries + manifest as release assets. | With the SDK publish |
| **`awesome-saule`** | RELEASE_PLAN's answer to discovery without an index. | With the package manager |

**Keep in the main repo**: all pipeline crates, `saule-cli`, `saule-lsp`,
`benchmarks/`, `tests/`, `examples/`. They release together, they break together,
and a single `cargo test` covering them is worth more than tidiness.

> **Caution on splitting.** Every repo split is permanent overhead: cross-repo
> version bumps, cross-repo CI, cross-repo breakage that no single PR can fix.
> With one maintainer, split only what *must* release separately. The list above
> is deliberately short for that reason, and `www` is the one where staying put
> is defensible.

### 9.6 Target topology

```
saule-lang/saule                 ← toolchain: 15 crates, tests, benchmarks, examples
├── crates/saule                 ← NEW  facade, the embedder's single dependency  [publish]
├── crates/saule-project         ← DONE saule.config, shared by CLI + LSP
├── crates/saule-test            ← NEW  the `saule test` runner
├── crates/saule-stdlib          ← LATER, split from saule-interpreter
├── crates/saule-db              ← DONE parse/imports/seed; typeck query blocked (9.2.1)
├── crates/saule-native-abi                                                       [publish]
├── crates/saule-sdk                                                              [publish]
├── crates/saule-export-macro                                                     [publish]
└── … the existing pipeline crates                                    [publish = false]

saule-lang/saule-engine          ← the graphics engine, consuming the published SDK
saule-lang/saule-package-template← the example every package author copies
saule-lang/vscode-saule          ← marketplace
saule-lang/intellij-saule        ← JetBrains marketplace
saule-lang/saule.nvim            ← plugin managers
saule-lang/awesome-saule         ← discovery, no index required
```

---

## 10. Sequenced roadmap

Ordered by *unblocking*, not by size. Each phase makes the next one possible.

### Phase 1 — Make it obtainable
RELEASE_PLAN steps 2–4, unchanged. Prove the release pipeline, ship the one-line
installer, publish the three editor plugins. **Nothing else matters until someone
who is not you can run `saule`.**

### Phase 2 — Make it usable
- ~~Parser error recovery + partial AST.~~ **Done** (`197ef4f`).
- ~~Fix the closure/scope cycle (§3.2).~~ **Done** — lambdas capture names, not
  frames. Regression-guarded by unit tests asserting the defining scope is not
  retained, plus a `tests/closure_capture.sau` fixture pinning the semantics.
- Build the cycle-report tool (§6.2 item 7). Now the highest-value memory work:
  the closure leak went unnoticed for the life of the project because nothing
  measures this, and the remaining user-authored cycles have the same problem.
- `saule test`.
- Runtime stack traces.
- Document the remaining data-structure cycle behaviour; add `weak`.

### Phase 3 — Make it extensible
- Publish `saule-native-abi`, `saule-export-macro`, `saule-sdk` with semver.
- Add `abi_version` to the manifest **and** a load-time check.
- ~~Extract `saule-project`; delete the duplicate config parser.~~ **Done.**
- Split `saule-engine-lib` into its own repo and rebuild it against the published
  SDK — the acid test that the path works for outsiders.
- Ship `saule-package-template`.

### Phase 4 — Make it a platform
RELEASE_PLAN steps 5–9: `saule add`, `saule publish`, the lockfile, native release
assets. Plus `saule doc`.

### Phase 5 — Make it stable
- `STABILITY.md`, the spec, the conformance suite.
- Every gate in §6.2: backwards-compatibility CI, diagnostic snapshots, fuzzing,
  benchmark regression, three-platform test matrix.
- Publish the `saule` facade + `EMBEDDING.md`.

### Phase 6 — Make it fast (**done ahead of schedule — read the caveat**)
The bytecode VM shipped and is the default engine, and tail calls came with it.
This phase was sequenced last, and deliberately: "it is the change most likely
to break semantics, and without the stability gates you will not know that it
did." Building it early meant paying for that warning some other way, and the
substitute was differential testing — the tree-walker kept in-tree as an oracle,
every fixture and every example project run under both engines and compared on
output rather than exit status. That is narrower than a conformance suite: it
proves the two engines agree, not that either matches a specification, because
there still is not one. Phase 5 is still owed.

Still open here: coroutines (§3.6), and the query layer (§9.2), which touches the
same foundations.

---

## Appendix A — raw measurements

### Benchmarks — both engines against PUC Lua, after Phase 5's peepholes

`REPS=5 python benchmarks/bench.py`, release build, Windows 11 Pro x86_64,
Lua 5.4.8 — the **same box and the same Lua** as the run below it, which is
what makes the two tables a before/after rather than two unrelated rows.
Seconds, minimum across reps; **compare ratios, not absolute times.** This
box was under more load for this run than for the one below — `lua`'s own
times are 10–25% higher across the board — which is exactly why the ratio
columns are the ones to read, and why the instruction counts further down
are the figure to quote.

The change measured here is §17's emission peepholes (`VM_TASKS.md`, Phase 5),
in six parts and **with no new opcodes**: fused comparison branches, operands
read where they already are, small integer literals folded into the
instruction, and no jump to the next instruction.

| bench | VM | tree-walker | lua | VM/lua | walker/lua | VM speedup |
|---|---:|---:|---:|---:|---:|---:|
| loop_arith | 0.190 | 0.657 | 0.071 | 2.7× | 9.4× | **3.46×** |
| fib | 0.101 | 0.352 | 0.050 | 2.0× | 7.1× | **3.49×** |
| array | 0.137 | 0.394 | 0.085 | 1.6× | 4.6× | **2.88×** |
| map | 0.389 | 0.499 | 0.478 | 0.8× | 1.0× | 1.28× |
| oop | 0.172 | 0.503 | 0.071 | 2.5× | 7.0× | **2.92×** |
| mandel | 0.144 | 0.507 | 0.062 | 2.3× | 8.3× | **3.52×** |
| strings | 0.110 | 0.179 | 0.066 | 1.7× | 2.7× | 1.63× |
| closure | 0.101 | 0.306 | 0.051 | 2.0× | 6.1× | **3.03×** |
| sort | 0.734 | 0.907 | 0.153 | 4.8× | 6.0× | 1.24× |
| startup | 0.032 | 0.031 | 0.036 | 0.9× | 0.9× | 0.97× |

**Instructions retired, `--profile-bytecode`, before Phase 5 → after.** This
is the honest measure of what Phase 5 did: it is a count, not a stopwatch,
and it does not move with machine load. Covers the emission peepholes and
the one superinstruction, `CASTUNWRAP`.

| bench | before | after | change |
|---|---:|---:|---:|
| loop_arith | 40,000,011 | 20,000,011 | **−50%** |
| mandel | 25,620,013 | 14,203,848 | **−45%** |
| fib | 11,827,257 | 7,713,431 | **−35%** |
| sort | 29,063,881 | 15,531,950 | **−47%** |
| closure | 10,000,012 | 8,000,012 | −20% |
| array | 11,000,017 | 9,000,017 | −18% |
| oop | 19,000,035 | 17,000,029 | −11% |

`loop_arith`'s inner loop is **6 instructions → 3**. `MOVE` — the
most-executed opcode in every benchmark this project has — fell by half or
more on the loop-heavy ones.

**`sort` is the one to read twice.** Its instruction count fell 47% and its
wall clock moved about 10% — most of that gap arriving with `CASTUNWRAP`,
which cut 30% of its instructions for 2.3% of its time. That is §20's
prediction with numbers attached: what is left in `map` and `sort` is
`TableObject` and the cost of crossing the engine boundary once per
comparison, not dispatch. Optimising dispatch further will not move them.

`map` and `sort` still barely move on the clock, for the reason §20 gave
before the VM was written: their time is inside `TableObject`, hashing and
comparison callbacks, not in dispatch. `sort` was 46% `CASTCHK` +
`UNWRAPNIL` in the profile before those two fused — and that is the
*program*, not the compiler: its comparator casts an untyped
parameter on every comparison, and the tree-walker does the same work.

### Benchmarks — both engines against PUC Lua, at the Phase 4 flip

`REPS=5 python benchmarks/bench.py`, release build (fat LTO, 1 codegen unit),
Windows 11 Pro x86_64, Lua 5.4.8. Seconds, minimum across reps; **compare
ratios, not absolute times.**

Two runs of `bench.py`, one per engine, each timing `lua` alongside it — the
two runs saw the same Lua times to within a millisecond on every benchmark,
which is what makes their columns comparable to each other and not just to
their own reference. The engine was selected with `SAULE_ENGINE`; all ten
benchmarks compile fully, so the VM column is a VM measurement and not a
fallback in disguise. All three engines were confirmed to print identical
output first (`bench.py check`).

| bench | VM | tree-walker | lua | VM/lua | walker/lua | VM speedup |
|---|---:|---:|---:|---:|---:|---:|
| loop_arith | 0.251 | 0.616 | 0.068 | 3.7× | 9.0× | **2.45×** |
| fib | 0.123 | 0.333 | 0.052 | 2.4× | 6.3× | **2.71×** |
| array | 0.144 | 0.355 | 0.082 | 1.8× | 4.3× | **2.47×** |
| map | 0.324 | 0.405 | 0.331 | 1.0× | 1.2× | 1.25× |
| oop | 0.191 | 0.482 | 0.072 | 2.7× | 6.6× | **2.52×** |
| mandel | 0.172 | 0.481 | 0.064 | 2.7× | 7.3× | **2.80×** |
| strings | 0.105 | 0.272 | 0.066 | 1.6× | 4.1× | **2.59×** |
| closure | 0.112 | 0.288 | 0.053 | 2.1× | 5.5× | **2.57×** |
| sort | 0.711 | 0.821 | 0.147 | 4.8× | 5.5× | 1.15× |
| startup | 0.033 | 0.033 | 0.040 | 0.8× | 0.8× | 1.00× |

Reading:

- **The two weakest cases became the two strongest gains.** `fib` (recursive
  calls) went 6.3× – 2.4× Lua and `loop_arith` 9.0× – 3.7×, because both
  were paying AST dispatch on every operation and that is exactly the cost a
  register VM deletes.
- **`map` and `sort` barely move** (1.25×, 1.15×), and they are the two
  benchmarks whose time is spent inside `TableObject` — hashing and comparison
  callbacks — rather than in dispatch. `VM_DESIGN.md` §20 predicted this before
  the VM was written. `map` was already 1.2× Lua under the tree-walker; there
  was nothing there for a VM to win.
- **`startup` is faster than Lua** and unchanged by the engine, since compiling
  a 3-line program costs nothing measurable. The front end still costs the user
  nothing.
- **LuaJIT is not in this table.** It was not installed on *this* measuring
  machine, and the 30–90× figure this document used to quote came from a
  different OS and architecture. Carrying it forward next to freshly measured
  columns would read as a comparison that was not made. It *is* installed on
  the macOS box, so the second table below has the column this one cannot.

### The same benchmarks on macOS arm64

`REPS=7 python3 benchmarks/bench.py`, release build, macOS arm64 (Darwin
25.6.0), **Lua 5.5.0** and **LuaJIT 2.1**. A separate machine and a separate
Lua from the table above, so this is a second measurement rather than a
correction to it — **do not average the two, and do not read a difference
between them as a regression.**

Both engines were timed in one run each via `SAULE_ENGINE`, and all ten
programs were confirmed to print identical output under the VM, the
tree-walker and Lua first (`bench.py check`). Two independent runs agreed
within ~1%.

| bench | VM | tree-walker | lua | luajit | VM/lua | walker/lua | VM/luajit | VM speedup |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| loop_arith | 0.179 | 0.452 | 0.061 | 0.013 | 2.9× | 7.4× | 13.8× | **2.53×** |
| fib | 0.104 | 0.294 | 0.023 | 0.005 | 4.5× | 12.8× | 20.8× | **2.83×** |
| array | 0.110 | 0.278 | 0.051 | 0.007 | 2.2× | 5.5× | 15.7× | **2.53×** |
| map | 0.334 | 0.401 | 0.316 | 0.075 | 1.1× | 1.3× | 4.5× | 1.20× |
| oop | 0.162 | 0.394 | 0.049 | 0.004 | 3.3× | 8.0× | 40.5× | **2.43×** |
| mandel | 0.120 | 0.398 | 0.040 | 0.009 | 3.0× | 10.0× | 13.3× | **3.32×** |
| strings | 0.082 | 0.133 | 0.041 | 0.017 | 2.0× | 3.2× | 4.8× | 1.62× |
| closure | 0.086 | 0.274 | 0.023 | 0.004 | 3.7× | 11.9× | 21.5× | **3.19×** |
| sort | 0.660 | 0.845 | 0.146 | 0.104 | 4.5× | 5.8× | 6.3× | 1.28× |
| startup | 0.007 | 0.006 | 0.003 | 0.002 | 2.3× | 2.0× | 3.5× | 0.86× |

**VM over the tree-walker: 1.20×–3.32×, geometric mean 2.19×** (excluding
`startup`, which is the control).

Reading, and where it differs from the Windows run:

- **The shape is the same, the spread is wider.** `map` (1.20×), `sort`
  (1.28×) and `strings` (1.62×) are the three that barely move, and they are
  the three whose time is spent inside `TableObject` — hashing, comparator
  callbacks, concatenation — rather than in dispatch. §20 predicted `map`
  before the VM existed. `strings` moved 2.59× on Windows and 1.62× here,
  which is the largest platform disagreement in the table and is worth a
  profile before anyone quotes either number as *the* figure.
- **`startup` is no longer faster than Lua.** 0.007 s against Lua's 0.003 s,
  where the Windows run had Saule ahead at 0.033 vs 0.040. The control is
  measuring process launch, and the two platforms' launch costs differ by an
  order of magnitude; nothing about the front end changed.
- **LuaJIT is 3.5×–40.5× ahead of the VM**, and that is the honest ceiling
  reference: it is a tracing JIT and this is an interpreter, so the gap is
  categorical rather than a list of missing optimizations. `oop` (40.5×) is
  where a JIT's inlining of field access shows most.

### Bytecode VM vs tree-walker, in-process

`cargo run --release -p saule-vm --example compare`, same machine. This one
runs both engines inside a single process, so there is no process-spawn floor
in the numbers and no `class Main` wrapper — and it **asserts the two engines
produced the same value** before it reports a timing.

| program | tree-walker | VM | speedup |
|---|---:|---:|---:|
| loop_arith | 397.5 ms | 148.6 ms | 2.68× |
| fib | 177.0 ms | 51.3 ms | **3.45×** |
| while_sum | 464.0 ms | 176.4 ms | 2.63× |
| call_heavy | 291.2 ms | 77.8 ms | **3.74×** |

The call-heavy programs gain most (3.45×, 3.74×) and gain more here than in
the table above, which is the process-spawn floor showing: `bench.py` measures
wall clock from `CreateProcess`, and on Windows that floor is ~33 ms of the
123 ms `fib` run. Net of it, `fib` is 3.1×. Both readings are honest — the
first is what a user waiting at a prompt experiences, the second is what the
engine actually does.

The same example on macOS arm64, three consecutive runs:

| program | tree-walker | VM | speedup |
|---|---:|---:|---:|
| loop_arith | 308–311 ms | 114–115 ms | 2.67×–2.73× |
| fib | 153 ms | 58–59 ms | 2.61×–2.63× |
| while_sum | 314 ms | 146 ms | 2.15×–2.16× |
| call_heavy | 276–278 ms | 79–83 ms | **3.33×–3.51×** |

**In-process: 2.15×–3.51×.** The call-heavy shape again gains most, and the
run-to-run spread on it (3.33×–3.51×) is larger than the ~3% noise floor the
rest of the table sits inside — so quote it as a range, not a point.

`VM_DESIGN.md` §21.3 gated Phase 2 on `loop_arith` and `fib` reaching **at least
2.5×** the tree-walker. Both clear it.

### Memory behaviour

Peak working set, sampled every 20 ms while the process ran;
`target/release/saule.exe` on Windows 11, `saule run <file>`. Every variant
allocates one 10-element `table<integer>` per iteration and differs only in how
it treats a closure. Sources are reproduced in §3.2.

| Variant | Iterations | Before | After |
|---|---:|---:|---:|
| Control — no closure | 3,000,000 | 7.5 MB | 7.5 MB |
| Lambda passed as an argument, not stored back | 3,000,000 | 7.6 MB | 7.6 MB |
| Lambda stored into the scope it captured | 3,000,000 | 2,468.7 MB (~860 B/iter) | **7.6 MB** |
| `Node` back-edge pair (`a.next = b; b.next = a`) | 1,000,000 | 1,282.8 MB (~1.3 KB/iter) | 1,282.8 MB |

The second row is the control that isolated the cause: closures were never the
problem, storing one into its own defining scope was. The third row is the fix
(§3.2); the fourth is the user-authored cycle that remains.

A fifth variant, a self-recursive local closure, is measured separately because
it was a language change as well as a memory one:

| Variant | Iterations | Capturing the self-name | Binding it per call |
|---|---:|---:|---:|
| `local down = fn(n) … down(n-1) … end` | 3,000,000 | 3,355.2 MB | **7.7 MB** |

A *named* nested `fn` declaration still cannot be measured: the resolver rejects
it (`undefined name`), because named functions are top-level only in Saule.

### Interpreter throughput, before vs after the capture change

`REPS=7 python benchmarks/bench.py`, release build, same machine. The rightmost
column is the honest read: a second run of the *unchanged* binary against itself
moved individual benchmarks by up to 3%, so anything at or under that is noise.

| bench | after | before | delta | noise floor (old vs old) |
|---|---:|---:|---:|---:|
| loop_arith | 0.603 | 0.593 | +1.7% | 1.8% |
| fib | 0.317 | 0.315 | +0.6% | 3.1% |
| array | 0.354 | 0.341 | +3.8% | 0.3% |
| map | 0.395 | 0.396 | −0.3% | 2.2% |
| oop | 0.452 | 0.438 | +3.2% | 0.2% |
| mandel | 0.436 | 0.433 | +0.7% | 0.4% |
| strings | 0.154 | 0.153 | +0.7% | 2.5% |
| closure | 0.251 | 0.237 | +5.9% | 0.4% |
| sort | 0.760 | 0.753 | +0.9% | 2.3% |
| startup | 0.027 | 0.027 | 0% | 0% |

`closure` is the one clearly outside the noise, which is what the design
predicts: creating a closure now walks its capture set and builds a flat scope
instead of cloning one `Rc`. Reading a variable — the far hotter operation — is
unchanged, which is why the rest sits in the noise.

### Crate sizes

| Crate | LOC | Files |
|---|---:|---:|
| saule-lsp | 20,754 | 55 |
| saule-interpreter | 15,046 | 61 |
| saule-typeck | 8,043 | 20 |
| saule-engine-lib | 6,583 | 26 |
| saule-parser | 2,995 | 13 |
| saule-fmt | 2,943 | 7 |
| saule-semantic | 2,565 | 13 |
| saule-cli | 1,514 | 8 |
| saule-sdk | 1,268 | 5 |
| saule-lexer | 1,103 | 4 |
| saule-docs | 1,015 | 6 |
| saule-ast | 865 | 6 |
| saule-export-macro | 502 | 1 |
| saule-wasm | 446 | 1 |
| saule-version | 417 | 2 |
| saule-native-abi | 314 | 1 |

Includes test modules and `tests/` directories.

### Safety surface

`unsafe` blocks are concentrated exactly where they should be: `saule-native-abi`
(13), `saule-sdk` (15), `saule-interpreter` (34, almost all in the native-host
callback table and dynamic loading), `saule-export-macro` (3, in generated code).
The pipeline crates — `saule-ast`, `saule-lexer`, `saule-parser`,
`saule-semantic`, `saule-typeck`, `saule-fmt` — contain **zero** `unsafe`. That
is a good boundary and it should be stated as a policy so it stays true.

### Test inventory

- 847 passing Rust tests (workspace minus `saule-engine-lib`), 0 failures, 5 ignored
- 919 `#[test]` functions across all crates
- 224 `.sau` fixtures: positives must run and exit 0, `tests/ui/*` must fail
- Doctests: 1 running (`output::capture`), 3 marked `ignore` in the ABI/SDK docs

The doctest count is the one weak spot in an otherwise strong test story — the
SDK's usage examples are `ignore`d, so the documented API is not compile-checked.
Making those real doctests would catch SDK API drift for free.
