# Running Saule in the browser

Plan for compiling the interpreter to WebAssembly so the
[playground](https://lauriszz123.github.io/saule/play/) can actually execute
code.

> **Status: complete.** Saule runs in the browser. All six stages are done and
> every playground example executes; the notes below record what was built and
> why, including the decisions that changed along the way. The one deliberate
> omission is `import`, which needs a virtual filesystem — see below.

## What we already know

The whole pipeline compiles for `wasm32-unknown-unknown` today. A
`cargo check -p saule-interpreter --target wasm32-unknown-unknown` produces
exactly three errors, all from one dependency:

```
dynamic_packages.rs:39   unresolved import `libloading::Library`
dynamic_packages.rs:437  cannot find type `Symbol` in `libloading`
native_host.rs:317       cannot find type `Library` in `libloading`
```

`std::fs`, `std::process` and `std::time` all *compile* for wasm — they fail at
runtime instead. So this is mostly a behaviour problem, not a porting one.

### What works in the browser, and what can't

**Works:** the full type system, classes, inheritance, interfaces, enums,
exhaustive `match`, closures, generics, null safety, `String` / `Math` /
`Table`, and printed output. Every one of the playground's eight examples.

**Can't:** `import` (module resolution needs a filesystem), native packages, and
the real-IO parts of `Os` / `Io`. The filesystem calls already degrade to
errors rather than panics, which is honest behaviour for a sandbox.

`import` is recoverable later by putting a virtual filesystem behind a trait in
`module.rs` — real fs natively, an in-memory map in the browser. Out of scope
for v1.

### Toolchain note

This machine has two Rust installs. The active `rustc` is Homebrew's, but
`rustup target add` installs the wasm std into the *rustup* toolchain, so cargo
picks up a `rustc` that has no wasm std and fails with a misleading
`can't find crate for core`. Until that is resolved (drop the Homebrew rust, or
add a `rust-toolchain.toml`), wasm builds need both pinned:

```sh
TC=~/.rustup/toolchains/stable-aarch64-apple-darwin
RUSTC=$TC/bin/rustc $TC/bin/cargo check -p saule-interpreter --target wasm32-unknown-unknown
```

---

## ~~Stage 1 — Feature-gate the native-package loader~~ ✅ Done

The only thing standing between the interpreter and a wasm build.

- Make `libloading` an optional dependency of `saule-interpreter`.
- Add a `native-packages` feature, **on by default**, so the CLI, LSP and every
  existing build are unaffected.
- Gate only the items that actually touch `libloading::Library` — the metadata
  half of `dynamic_packages` (discovery, manifest parsing, `export_names`,
  `seed_classes`) stays available, because `module.rs`, `stdlib/mod.rs` and the
  LSP all depend on it.
- With the feature off, `build_exports` returns a clear `ImportError` instead of
  silently pretending the package loaded.

**Done when:** `cargo check -p saule-interpreter --target wasm32-unknown-unknown
--no-default-features` succeeds, and the native build plus test suite are
unchanged.

**Outcome.** The interpreter now compiles for `wasm32-unknown-unknown` with
zero errors *and zero warnings*. `libloading` is optional behind the
`native-packages` feature (default on). Gated: the `LIBS` cache,
`load_library`, `pick_binary`, `build_class`, `make_native`, `call_native`,
`spread_multi_return`, and the whole `native_host` module — its only purpose is
handing a host-callback table to a dlopen'd package, so without the feature
there is nothing to hand it to. `build_exports` has a second implementation
that returns a clear `ImportError`. Manifests are still discovered and their
type signatures still register, so a program importing a native package
type-checks identically on both targets; it just cannot run.

## ~~Stage 2 — Swappable output sink~~ ✅ Done

On `wasm32-unknown-unknown` there is no stdout: `println!` is discarded. Without
this the playground would run a program and display nothing.

- Add `saule-interpreter::output` with a `Sink` trait, a thread-local current
  sink, and a `CaptureSink` that records `(stream, text)` chunks.
- Route every write through it. There are only nine sites: `print` / `println` /
  `printf` in `stdlib/core.rs`, and `Io.write` / `Io.stdout` / `Io.stderr` /
  the two flushes in `stdlib/io.rs`.
- Default behaviour with no sink installed stays exactly what it is today —
  straight to real stdout/stderr.

Worth doing regardless of wasm: it lets the test suite assert on program output
directly instead of shelling out to the binary.

**Done when:** output is capturable in-process, and the existing tests pass
untouched.

**Outcome.** `saule_interpreter::output` provides `Sink`, `write`, `flush`,
`with_sink`, `capture` and a `CaptureSink` that records `(stream, text)` chunks
in emission order, coalescing consecutive same-stream writes. All nine write
sites now route through it. With no sink installed the behaviour is byte-for-byte
what it was — verified against the CLI with `od -c`.

`capture` restores the previous sink through a `Drop` guard, so a panicking
program cannot strand a sink on the thread; there is a test for exactly that,
and for nesting. Six integration tests in
`crates/saule-interpreter/tests/output_capture.rs` run real programs through the
full pipeline and assert on what they printed, including stdout/stderr
separation — which is the capability the browser runtime is built on.

Whole workspace: 36 test binaries, 0 failures.

## ~~Stage 3 — Stop the runtime panics~~ ✅ Done

`Instant::now()` and `SystemTime::now()` **panic** on `wasm32-unknown-unknown`,
which traps the whole module. These are reachable from ordinary Saule code:

| Function | Problem |
|---|---|
| `Os.clock` | `Instant::now()` — panics |
| `Os.time`, `Os.date` | `SystemTime::now()` — panics |
| `Os.sleep` | `thread::sleep` — no-op or panic |
| `Os.exit` | `process::exit` — traps the module |
| `Os.pid` | `process::id()` |

Guard each behind `cfg(target_arch = "wasm32")`, backed by `js_sys::Date::now()`
/ `performance.now()` where a real value makes sense, and a stub that returns an
error where it does not. `Os.exit` should unwind with a distinguishable
"program exited" signal rather than killing the module.

**Outcome.** Solved with an injectable `saule_interpreter::platform::Platform`
rather than `cfg` branches sprinkled through `os.rs` — the same shape as
[stage 2](#stage-2--swappable-output-sink)'s output sink, and for the same
reason: it keeps `wasm-bindgen` out of the interpreter entirely. Stage 4
installs a JS-backed platform instead of the interpreter importing `js_sys`.

The trait covers `unix_time_secs`, `monotonic_secs`, `sleep`, `pid` and `exit`.
Every method defaults to "unavailable", so an embedder implements only what its
host can do. Natively a `NativePlatform` is compiled in and **nothing changes**;
on wasm the default reports everything unavailable, so `Os.time()` returns a
clear error instead of trapping.

Behaviour on a host with no facilities:

| Call | Result |
|---|---|
| `Os.time`, `Os.clock` | error naming the function — honest, not a wrong number |
| `Os.date("%Y", 0)` | works; only the implicit "now" needs a clock |
| `Os.date("%Y")` | error |
| `Os.sleep` | error, rather than silently not sleeping and letting a paced loop spin |
| `Os.exit(3)` | unwinds with `program exited with code 3`; the code is parked for `platform::take_exit()` so an embedder can tell a deliberate exit from a crash |
| `Os.pid` | `0` — "no process" is truthful, and callers use it for uniqueness, not control |
| `Os.tmpname` | stays total via a per-thread counter when there is no clock or pid |

`Os.execute` needed nothing: `Command::status()` already returns `Err` on wasm
rather than panicking, which the existing code maps to exit code `-1`.
`Os.fsInfo`'s `UNIX_EPOCH` use is not a clock read — it converts a timestamp the
filesystem already returned — so it stays as it was.

Ten integration tests in `crates/saule-interpreter/tests/sandboxed_platform.rs`
install a do-nothing platform (exactly what a bare wasm host looks like) and pin
all of the above; they run on any target. `platform.rs` carries five more unit
tests, including that a panicking program cannot strand an installed platform.

Whole workspace: 37 test binaries, 0 failures. The interpreter compiles for
`wasm32-unknown-unknown` with zero errors and zero warnings.

## ~~Stage 4 — The `saule-wasm` crate~~ ✅ Done

- New `crates/saule-wasm`, `crate-type = ["cdylib"]`, using `wasm-bindgen`.
- Export `run(source: &str) -> String`, returning JSON matching the `RunResult`
  shape already defined in `www/src/lib/runtime.ts`.
- Map diagnostics to `{severity, phase, message, span: {start, end}, help}`.
  Every error type already carries a span, so the editor can underline the
  offending range rather than printing a bare message.

**Outcome. Saule runs in WebAssembly.** Verified end to end by initialising the
built module in Node and executing real programs: classes, inheritance, enums
with payloads, exhaustive `match`, the type checker, runtime diagnostics with
byte spans, the injected JS clock, and `Os.exit`.

**Size: 1.1 MB raw, ~333 KB gzipped** — better than the 500–800 KB estimated.
`wasm-opt` was not installed, so installing binaryen should shave another
15–25%.

Design notes:

- `wasm-bindgen` and `js-sys` are **target-scoped** dependencies, so a native
  `cargo test --workspace` neither builds nor links them. The JSON-shaping
  logic is therefore ordinary Rust with 14 unit tests, no browser needed.
- Diagnostics are read through miette's `Diagnostic` trait — `labels()` for the
  span, `help()` for the hint — rather than by matching each error enum. Every
  phase maps identically and a new error variant needs no work here.
- Unlike `check_and_run`, which stops at the first diagnostic, this reports
  **every** semantic and type error in one pass. A playground that surfaces one
  error per run is a poor way to learn a type system.
- `Os.exit(n)` arrives as an unwind (stage 3), so the crate pairs it with
  `platform::take_exit()` and reports `ok: true` — a deliberate exit is a normal
  ending, not a fault. Output printed before a *genuine* failure is preserved
  too, so a program that crashes halfway does not look like it never ran.
- `BrowserPlatform` implements only `unix_time_secs` / `monotonic_secs`, both
  from `js_sys::Date::now()` — no `web-sys` dependency, and it works in a Worker
  as readily as on the main thread. `sleep`, `pid` and `exit` keep the trait's
  "unavailable" defaults, which is exactly what stage 5 wants.

`www/scripts/build-wasm.sh` compiles, runs `wasm-bindgen --target web`, and
optionally `wasm-opt`. Output goes to `www/src/lib/saule_wasm/` (gitignored — a
1.1 MB binary rebuilt on every deploy belongs in CI output, not history).

Two toolchain traps the script now handles, both of which present as a
misleading `can't find crate for core`:

1. The host triple is `aarch64`, but `uname -m` prints `arm64` — so locating the
   toolchain directory by hand silently fails.
2. `rustup run` is **not** sufficient. It fixes the command it launches, but
   cargo then spawns `rustc` *by name* and picks up whatever PATH offers first,
   landing back on Homebrew's. `RUSTC` has to be exported explicitly, via
   `rustup which rustc`.

## ~~Stage 5 — Web Worker + Stop button~~ ✅ Done

A wasm module cannot be interrupted from outside, so `while true do end` would
hang the tab permanently with no recourse.

- Run the module in a Web Worker; the main thread stays responsive and the
  worker can be terminated.
- Add a Stop button to the playground, enabled while running.
- Belt and braces: an optional step budget in the eval loop that aborts with a
  "program ran too long" diagnostic.

Non-negotiable before this ships publicly.

**Outcome.** `www/src/lib/saule-worker.ts` owns the module; `runtime.ts` drives
it and parses results on the main thread, so a malformed payload surfaces there
rather than as a dead worker. Verified in the browser: with `while true do end`
running, the page stayed fully responsive (JS evaluated mid-loop) and **Stop**
recovered it at 2.0 s, after which a normal program ran again immediately.

The 10-second backstop is separate and also verified — it fired on its own
during testing with a distinct message ("An infinite loop, perhaps?"), so the
two paths are distinguishable to the user.

<kbd>Esc</kbd> stops as well: when a loop runs away the mouse may be nowhere
near the button.

The step-budget idea was dropped. Terminating the worker is unconditional and
costs nothing at runtime, whereas a budget checked in the eval loop would slow
every program to guard against a rare mistake.

## ~~Stage 6 — Wire it into the site~~ ✅ Done

- Build with `wasm-pack` / `wasm-bindgen-cli`; `opt-level = "z"` plus
  `wasm-opt`. Expect roughly 1–2 MB raw, ~500–800 KB gzipped.
- Replace the body of `load()` in `www/src/lib/runtime.ts` with the dynamic
  import already sketched in its doc comment, so the module only downloads when
  someone opens `/play/`.
- Flip `RUNTIME_AVAILABLE` to `true` and drop the "coming soon" notice from
  `www/src/content/docs/play.mdx`.
- Add the wasm build to `.github/workflows/deploy-www.yml`, and to
  `www/scripts/deploy-gh-pages.sh` for the local deploy path.

**Outcome.** All eight playground examples run in the browser — classes,
generics, interfaces, pattern matching, null safety, FizzBuzz — in 1–9 ms each
after the module has loaded. Diagnostics render with their phase label and
miette's help text, and output printed before a runtime fault is preserved.

Rather than a bare dynamic `import()`, the module is imported by the worker and
Vite emits it as a hashed asset (`?url`), so it is fetched rather than inlined
as base64 and stays cacheable. The worker is not spawned until the first Run, so
opening `/play/` costs nothing extra.

`build:wasm` is wired into `predev` and `prebuild`, which means the site cannot
be built against a stale or missing module — a fresh clone just works, given a
Rust toolchain. Verified by deleting `src/lib/saule_wasm/` and `dist/` and
running `npm run build` clean.

CI installs `wasm-bindgen-cli` at the version read out of `Cargo.lock`, since a
mismatch with the crate is a hard schema error. `deploy-www.yml` now also
triggers on `crates/**` — the playground ships a build of the interpreter, so a
change to the language is a change to the site.

## Rough effort

| Stage | Effort |
|---|---|
| ~~1 — Feature-gate the loader~~ | ✅ done |
| ~~2 — Output sink~~ | ✅ done |
| ~~3 — Runtime panics~~ | ✅ done |
| ~~4 — `saule-wasm` crate~~ | ✅ done |
| ~~5 — Worker + Stop~~ | ✅ done |
| ~~6 — Site integration~~ | ✅ done |

About two focused days to something that runs most programs, four to something
solid.
