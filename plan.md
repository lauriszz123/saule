# Running Saule in the browser

Plan for compiling the interpreter to WebAssembly so the
[playground](https://lauriszz123.github.io/saule/play/) can actually execute
code. Today the playground's editor, examples and link-sharing all work; only
execution is stubbed, behind `www/src/lib/runtime.ts`.

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

## Stage 3 — Stop the runtime panics

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

## Stage 4 — The `saule-wasm` crate

- New `crates/saule-wasm`, `crate-type = ["cdylib"]`, using `wasm-bindgen`.
- Export `run(source: &str) -> String`, returning JSON matching the `RunResult`
  shape already defined in `www/src/lib/runtime.ts`.
- Map diagnostics to `{severity, phase, message, span: {start, end}, help}`.
  Every error type already carries a span, so the editor can underline the
  offending range rather than printing a bare message.

## Stage 5 — Web Worker + Stop button

A wasm module cannot be interrupted from outside, so `while true do end` would
hang the tab permanently with no recourse.

- Run the module in a Web Worker; the main thread stays responsive and the
  worker can be terminated.
- Add a Stop button to the playground, enabled while running.
- Belt and braces: an optional step budget in the eval loop that aborts with a
  "program ran too long" diagnostic.

Non-negotiable before this ships publicly.

## Stage 6 — Wire it into the site

- Build with `wasm-pack` / `wasm-bindgen-cli`; `opt-level = "z"` plus
  `wasm-opt`. Expect roughly 1–2 MB raw, ~500–800 KB gzipped.
- Replace the body of `load()` in `www/src/lib/runtime.ts` with the dynamic
  import already sketched in its doc comment, so the module only downloads when
  someone opens `/play/`.
- Flip `RUNTIME_AVAILABLE` to `true` and drop the "coming soon" notice from
  `www/src/content/docs/play.mdx`.
- Add the wasm build to `.github/workflows/deploy-www.yml`, and to
  `www/scripts/deploy-gh-pages.sh` for the local deploy path.

---

## Rough effort

| Stage | Effort |
|---|---|
| ~~1 — Feature-gate the loader~~ | ✅ done |
| ~~2 — Output sink~~ | ✅ done |
| 3 — Runtime panics | ~3–5h |
| 4 — `saule-wasm` crate | ~4–6h |
| 5 — Worker + Stop | ~4–6h |
| 6 — Site integration | ~3–4h |

About two focused days to something that runs most programs, four to something
solid.
