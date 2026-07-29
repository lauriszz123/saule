# Saule website

The documentation site and playground, built with [Astro] + [Starlight] and
deployed to GitHub Pages at <https://lauriszz123.github.io/saule/>.

```sh
npm install
npm run dev        # http://localhost:4321/saule/
```

## How it fits together

The site is generated from the repository wherever it can be, so documentation
cannot drift from the thing it documents.

| On the site | Comes from | Kept in sync by |
|---|---|---|
| Language Guide | `README.md` | `npm run sync-docs` |
| Standard Library | `DOCS.md` | `npm run sync-docs` |
| Quick Reference | `README.md` § Quick Reference | `npm run sync-docs` |
| Examples | `examples/*/` — real source, read verbatim | `npm run sync-docs` |
| Code highlighting | `editors/vscode/syntaxes/saule.tmLanguage.json` | read at build time |
| CLI reference, guides, landing page | hand-written in `src/content/docs/` | — |

**Never hand-edit** anything under `src/content/docs/{language,stdlib,examples}`
or `src/content/docs/reference/quick-reference.md` — those directories are wiped
and regenerated. Edit `README.md`, `DOCS.md`, or the example project instead,
then re-run the sync.

Highlighting deserves a note: Shiki reads the VS Code extension's TextMate
grammar directly (see `src/lib/saule-grammar.mjs`), so the website and the
editor can never disagree about what a keyword is. Adding a keyword to
`saule.tmLanguage.json` updates both.

## Scripts

| Command | What it does |
|---|---|
| `npm run dev` | Dev server with hot reload |
| `npm run build` | Production build into `dist/` |
| `npm run preview` | Serve the production build locally |
| `npm run sync-docs` | Regenerate docs pages from `README.md`, `DOCS.md`, `examples/` |
| `npm run check-samples` | Compile and run every hand-written Saule sample on the site |

`check-samples` needs the compiler. It uses `../target/release/saule` if
present, `../target/debug/saule` otherwise, and finally `saule` on `PATH`:

```sh
cargo build --release -p saule-cli
npm run check-samples
```

It only checks samples that are meant to be complete programs — the playground
examples and the ```saule blocks on the hand-written pages, listed in
`HAND_WRITTEN` at the top of `scripts/check-samples.mjs`. Snippets generated
from `README.md` are excluded, because most are illustrative fragments that
were never meant to compile standalone.

## The playground

Saule runs in the browser, compiled to WebAssembly from the same interpreter
the CLI uses.

| File | Role |
|---|---|
| `src/components/Playground.astro` | Editor, examples, output pane, Run/Stop |
| `src/lib/runtime.ts` | Execution boundary — spawns the worker, parses results |
| `src/lib/saule-worker.ts` | Runs programs off the main thread |
| `crates/saule-wasm` | The Rust side: `run(source) -> String` returning JSON |

Build the module with:

```sh
npm run build:wasm
```

`npm run dev` and `npm run build` both do this automatically (`predev` /
`prebuild`), so the only time you need it by hand is after `cargo clean`. It
needs a Rust toolchain, the `wasm32-unknown-unknown` target, and
`wasm-bindgen-cli` at the version pinned in `Cargo.lock` — the script says so
explicitly if any is missing.

Output goes to `src/lib/saule_wasm/` and is **gitignored**: a 1.1 MB binary
rebuilt on every deploy belongs in CI output, not in history.

### Why a Web Worker

A WebAssembly module cannot be interrupted from the outside. Run
`while true do end` on the main thread and the page is gone — no repaint, no
Stop button, no way back but closing the tab. In a worker the page stays
responsive and `terminate()` is an unconditional kill, which is what makes
**Stop** (and <kbd>Esc</kbd>) possible. A 10-second timeout stops anything
still running, as a backstop for someone who wanders off.

Terminating discards the module, so the next run re-initialises it — a fair
price for being able to escape an infinite loop at all.

### Sandbox

Programs run in single-file mode, as with `saule run file.sau`. `import` is
unavailable (module resolution needs a filesystem) and so are the real-IO parts
of `Os`/`Io`; both report ordinary diagnostics rather than crashing. Everything
else — the full type system, classes, interfaces, enums, pattern matching,
closures, `String`/`Math`/`Table` — behaves exactly as it does locally.

## Deployment

There are two paths, and GitHub's **Settings → Pages → Source** picks between
them. Use one or the other — the setting cannot serve both.

### Via GitHub Actions (preferred)

`.github/workflows/deploy-www.yml` builds and publishes on every push to `main`
that touches the site or any of its sources. Set **Source → GitHub Actions**
once, and that is the whole setup.

### Locally, to a `gh-pages` branch (no CI required)

```sh
www/scripts/deploy-gh-pages.sh --dry-run   # build and stage, don't push
www/scripts/deploy-gh-pages.sh             # build, commit, push
```

Then set **Source → Deploy from a branch → `gh-pages` / (root)**.

Use this when Actions cannot run — an account billing lock, or just shipping
without CI. It produces the identical site; your machine does the building
instead of a runner. Note it builds from your **working tree**, not from
`HEAD`, so uncommitted edits get published (the script warns when the tree is
dirty).

One thing this path needs that the Actions path does not: a `.nojekyll` file.
Branch-based Pages runs the published files through Jekyll, which silently
drops directories whose names begin with an underscore — and Astro puts all of
its CSS and JS in `_astro/`. Without it the site loads as unstyled HTML with
dead scripts. The script creates the file itself, which is why it is not
committed under `public/`.

`base` and `site` live in `site.config.mjs`, shared by the Astro config and the
sync script. Moving to a custom domain means setting `site` to it and dropping
`base`.

[Astro]: https://astro.build
[Starlight]: https://starlight.astro.build
