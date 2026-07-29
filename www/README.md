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

`src/components/Playground.astro` is the editor UI; `src/lib/runtime.ts` is the
execution boundary.

**The browser runtime does not exist yet.** `runtime.ts` currently reports that
and nothing runs. Everything else — editing, highlighting, examples, sharing a
program by URL — works today and needs no changes when the runtime lands.

To finish it, build `crates/saule-wasm` targeting `wasm32-unknown-unknown` with
`wasm-bindgen`, exposing `run(source) -> string` (JSON matching the `RunResult`
shape in `runtime.ts`), then replace the body of `load()` with the dynamic
import sketched in its doc comment and flip `RUNTIME_AVAILABLE` to `true`. The
known blockers in the interpreter:

| Blocker | Where | Fix |
|---|---|---|
| `libloading` / dynamic native packages | `dynamic_packages.rs`, `native_host.rs` | Cargo feature, default-on, off for wasm |
| `print!`/`println!` write to real stdout | `stdlib/core.rs` | Redirect through a swappable writer so output can be captured |
| `std::fs` in module loader and `Io`/`Os` | `module.rs`, `stdlib/io.rs`, `stdlib/os.rs` | In-memory virtual filesystem behind a trait |
| `Instant`/`SystemTime`, `process::exit`, `Command` | `stdlib/os.rs` | `js_sys` shims and error-returning stubs |

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
