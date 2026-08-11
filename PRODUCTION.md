# Saule — Production Readiness Analysis

An assessment of what Saule is today, what it would take to become a language
ordinary people use for real work, how you would *know* it is stable, how hard
it is to embed in another project, how hard it is to write a native library for
it, and how the code should be split across crates and repositories.

Everything below is measured against the working tree at `46e810e` (main,
2026-08-10). Numbers were produced by running the suites and benchmarks on this
machine (macOS 25.6, arm64), not estimated.

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
| Is my editor going to work? | Until you type a syntax error, at which point every feature stops. |
| How fast is it? | 5–11× slower than PUC Lua, 30–90× slower than LuaJIT. |
| Is it stable? | Nothing states what "stable" means, and nothing enforces it. |

None of those are research problems. All of them are work, and the sequencing
matters more than the total volume — see [§10](#10-sequenced-roadmap).

The three things that most change Saule's trajectory, in order:

1. **Parser error recovery.** Right now `saule_parser::parse` returns
   `Result<Module, ParseError>` and bails on the first error
   ([lib.rs:52](crates/saule-parser/src/lib.rs:52)). A half-typed file produces
   one diagnostic and no AST, so the LSP loses completion, hover, and signature
   help exactly when the user needs them. This is the single largest gap between
   "an editor plugin exists" and "the editor feels good", and everything in the
   LSP crate — 20,754 lines, the largest crate in the workspace — is built on
   top of an input that vanishes under normal typing.
2. **Publishing `saule-native-abi` / `saule-sdk` / `saule-export-macro` to
   crates.io.** Today they are path dependencies inside this workspace, so
   "write a native library for Saule" means "clone the compiler." That is a hard
   stop on third-party native packages, which is the ecosystem's growth path.
3. **A stability contract.** Not a version number — a written statement of what
   may change and what may not, with tests that fail when it does. §6 proposes
   one.

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

This is a real fork in the road, and it should be an explicit decision rather
than an inherited default:

- **Accept it, document it, provide the tools.** Add a `weak` reference type to
  the language, document the leak, and ship a way to observe it (a
  `--stats` flag reporting live `Rc` counts at exit). This is the Swift/ObjC
  answer. Cheapest, and honest.
- **Add a cycle collector.** A trial-deletion collector over `Rc` (the
  Bacon–Rajan algorithm) is the standard retrofit and does not require rewriting
  every value. Substantial work, contained blast radius.
- **Move to a tracing GC.** Correct in the long run, and effectively a rewrite of
  the value layer plus every native boundary.

There is also an environment recycling pool ([env.rs:44](crates/saule-interpreter/src/env.rs:44),
[recycle.rs](crates/saule-interpreter/src/recycle.rs)) — a nice optimisation, and
independent of the above.

### 3.3 Execution model

**A tree-walking interpreter.** `eval` calls `exec` calls `eval` across the AST,
with no bytecode, no IR, and no JIT. Consequences:

- **Performance is what a tree-walker gives you.** Measured against PUC Lua on
  the repo's own benchmark suite: 5.1×–10.9× slower on everything except
  `map` (1.2×, dominated by hashing) and `startup` (level with Lua, which is
  genuinely good — parse and typecheck cost nothing perceptible). Against
  LuaJIT the gap is 30–90×. Full table in [Appendix A](#appendix-a--raw-measurements).
- **Native stack depth is the recursion limit.** `MAX_EVAL_DEPTH = 10_000`
  ([eval/mod.rs:35](crates/saule-interpreter/src/eval/mod.rs:35)) converts what
  would be a SIGSEGV into a catchable `RuntimeError::StackOverflow`. Commit
  `60ccc75` shows the depth of thought here — the main thread gets a 512 MiB
  stack via a linker flag on macOS because AppKit forbids running the interpreter
  on a spawned thread. This is careful work.
- **No tail calls**, so recursive-loop idioms from Lua will hit the depth limit.

Whether the tree-walker is a problem depends entirely on the target. For scripting
a game's logic, config, or tooling, 7× Lua is fine. For anything compute-bound it
is not, and a bytecode VM is the standard next step. **Decide the target
workload before optimising**, because a bytecode compiler is a multi-month
project that also invalidates parts of the LSP's story.

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
  registries. This is correct but expensive: the LSP's own comment measures
  **~27 ms in the seed versus ~0.4 ms for lex + parse + all analysis combined**
  on `examples/UI Project` ([seed_cache.rs](crates/saule-lsp/src/server/seed_cache.rs)).
  A memo cache was added to make the editor usable. That ratio — 98% of a request
  spent re-deriving what did not change — is the signature of a compiler that
  wants to be query-based, and it will get worse with project size.
- **`saule.config` is parsed twice**, once in
  [saule-cli/src/project.rs](crates/saule-cli/src/project.rs) and once in
  [saule-lsp/src/workspace.rs](crates/saule-lsp/src/workspace.rs), the second
  with a comment explaining that the duplication was chosen over inverting the
  dependency direction. Correct diagnosis, wrong fix — the answer is a shared
  crate ([§9](#9-crate-and-repository-topology)).

### 3.6 Concurrency model

There is none. No coroutines (`thread` is a reserved, unimplemented type name),
no threads, no async, no channels. The interpreter is `Rc`-based and explicitly
thread-confined; every registry is `thread_local!`.

For a Lua-lineage language this is a notable absence — coroutines are one of
Lua's defining features and the idiom people bring with them. It is also the
feature most entangled with the tree-walking design: implementing real coroutines
on a recursive `eval` requires either OS threads with handoff, stack copying, or
a bytecode VM with an explicit frame stack. **Coroutines are effectively a vote
for a bytecode VM**, and should be decided together with §3.3.

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
| LSP robustness | Dies on any syntax error — no recovery | **D** |
| Runtime performance | 5–11× PUC Lua, 30–90× LuaJIT | **C** |
| Memory management | Refcount, cycles leak, no tooling to see it | **C−** |
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
3. **Parser gives up on the first error.** No recovery, no partial AST, one
   diagnostic per compile. In the LSP this means every feature — completion,
   hover, signature help, symbols — goes dark the moment a character is
   half-typed, which is most of the time while writing code.
   [diagnostics.rs:100](crates/saule-lsp/src/server/diagnostics.rs:100) shows the
   early return. *Fixing this is the highest-leverage change in the repo.*
4. **Editor plugins are unpublished.** All three exist and work; none are on a
   marketplace. A language you must sideload editor support for is a language
   people bounce off.

### Tier 1 — blocks serious use

5. **No test runner.** There is no `saule test`, no assertion library beyond the
   prelude `assert`, no test discovery, no reporting. Nobody writes production
   code in a language they cannot test. This is cheap to build relative to its
   impact.
6. **No stack traces on runtime errors.** See §3.4.
7. **Reference cycles leak.** See §3.2. At minimum: document it, add `weak`, and
   ship a way to see it.
8. **No debugger.** No DAP implementation. Print debugging only.
9. **Performance.** Acceptable for scripting; disqualifying for compute. Decide
   the target workload, then decide whether a bytecode VM is on the roadmap.
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
19. **The LSP re-derives types.** `exprty.rs` was written specifically to
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
| `saule-parser` | 2,995 | Recursive descent | Correct crate; needs recovery (§5.3). |
| `saule-semantic` | 2,565 | Resolution, registries, flow, field-init | Correct. |
| `saule-typeck` | 8,043 | Types, nullability, generics, exhaustiveness | Correct. |
| `saule-interpreter` | 15,046 | Tree-walker, stdlib, modules, native hosting | **Too broad — see 9.2.** |
| `saule-fmt` | 2,943 | Formatter | Correct, well-isolated. |
| `saule-docs` | 1,015 | `---` doc-comment extraction | Correct; under-used (§5.13). |
| `saule-cli` | 1,514 | `saule` binary | Correct. |
| `saule-lsp` | 20,754 | Language server | **Largest crate; see 9.3.** |
| `saule-native-abi` | 314 | Frozen C ABI | Correct. **Must be published.** |
| `saule-sdk` | 1,268 | Package-authoring SDK | Correct. **Must be published.** |
| `saule-export-macro` | 502 | `#[saule_export]` | Correct. **Must be published.** |
| `saule-version` | 417 | Build-time version resolution | Correct, clever, self-contained. |
| `saule-wasm` | 446 | Playground bindings | Correct. Candidate for its own repo. |
| `saule-engine-lib` | 6,583 | Graphics engine example | **Does not belong here — see 9.5.** |

The split is, on the whole, better than most language projects manage. The
pipeline crates are cleanly layered and the dependency graph flows one way. What
follows are the changes worth making.

### 9.2 New crates to extract

**`saule-project` — extract now, highest value.**
Owns `saule.config` parsing, project discovery, `src_dirs`/`dependencies`
resolution, and `ProjectInfo`. Today this logic is **implemented twice** — in
[saule-cli/src/project.rs](crates/saule-cli/src/project.rs) and in
[saule-lsp/src/workspace.rs](crates/saule-lsp/src/workspace.rs) — and the LSP's
copy carries a comment explaining that duplication was chosen because the CLI's
parser is private and depending on the CLI would invert the dependency direction.
That reasoning is right and the conclusion is wrong: the answer is a third crate
both depend on. Bonus: `saule-lsp` currently depends on the entire
`saule-interpreter` *just* to reach `project::ProjectInfo`. Extracting removes
that edge. Two config parsers that must agree and are separately maintained is a
bug waiting for its moment.

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

**`saule-db` / query layer — the strategic one.**
The 27 ms-vs-0.4 ms seed measurement (§3.5) and the LSP's duplicated inference
(§5.19) are the same problem from two directions: there is no incremental,
memoised layer between "files on disk" and "answers about the code". A
`salsa`-style query crate — parse, resolve, typecheck as memoised queries keyed
on file revision — would let both the CLI and the LSP ask the same questions and
get the same answers, and would delete both the ad-hoc seed cache and the ~800
lines of LSP-local inference. **This is the largest single architectural
improvement available**, and it should be a deliberate project, not a drive-by.

**`saule-syntax` — only if the parser gets recovery.**
An error-tolerant, lossless CST (rowan-style) is a different data structure from
the current `Module`, and if it lands it should be its own crate rather than
bolted onto `saule-parser`. Consider this the concrete plan for §5.3.

### 9.3 Crates to split

**`saule-lsp` (20,754 LOC) is doing three jobs**: protocol plumbing
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
├── crates/saule-project         ← NEW  saule.config, shared by CLI + LSP
├── crates/saule-test            ← NEW  the `saule test` runner
├── crates/saule-stdlib          ← LATER, split from saule-interpreter
├── crates/saule-db              ← LATER, incremental query layer
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
- Parser error recovery + partial AST. The highest-leverage change in the repo.
- `saule test`.
- Runtime stack traces.
- Document the cycle-leak behaviour; add `weak`.

### Phase 3 — Make it extensible
- Publish `saule-native-abi`, `saule-export-macro`, `saule-sdk` with semver.
- Add `abi_version` to the manifest **and** a load-time check.
- Extract `saule-project`; delete the duplicate config parser.
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

### Phase 6 — Make it fast (decide first)
Bytecode VM, and with it coroutines and tail calls. **Do not start this before
Phase 5**: it is the change most likely to break semantics, and without the
stability gates you will not know that it did. It is also the phase to revisit
the query layer (§9.2), since both touch the same foundations.

---

## Appendix A — raw measurements

### Benchmarks

`REPS=3 python3 benchmarks/bench.py`, release build (fat LTO, 1 codegen unit),
macOS 25.6 arm64. Seconds; compare ratios, not absolute times.

| bench | saule | lua | luajit | saule/lua |
|---|---:|---:|---:|---:|
| loop_arith | 0.454 | 0.064 | 0.014 | 7.0× |
| fib | 0.259 | 0.024 | 0.006 | 10.9× |
| array | 0.271 | 0.053 | 0.007 | 5.1× |
| map | 0.420 | 0.364 | 0.085 | 1.2× |
| oop | 0.375 | 0.050 | 0.004 | 7.6× |
| mandel | 0.348 | 0.040 | 0.008 | 8.8× |
| strings | 0.124 | 0.041 | 0.016 | 3.0× |
| closure | 0.197 | 0.023 | 0.004 | 8.5× |
| sort | 0.663 | 0.145 | 0.104 | 4.6× |
| startup | 0.003 | 0.003 | 0.003 | 1.2× |

Reading: the recursive-call path (`fib`, 10.9×) and raw arithmetic loops
(`loop_arith`, 7.0×) are the weakest, which is exactly what a tree-walker
predicts — every operation pays AST dispatch. `map` at 1.2× shows the table
implementation is competitive when hashing dominates. **`startup` level with Lua
is a real achievement** and means the front end costs the user nothing.

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
