[← The app shell](05-app-shell.md) · [Index](README.md) · [Testing →](07-testing.md)

# 6. Build order

Ten milestones. Each one **ends with something you can run and look at** —
that is the ordering principle, and it is why the parser gets finished before
a window exists rather than both growing at once.

| # | Milestone | You can | Rough size |
|---|---|---|---|
| 0 | [Make UIKit a package](#milestone-0--make-uikit-a-package) | ✅ **done** — `saule run "examples/UI Project"` works against the package | — |
| 1 | [Blocks, headless](#milestone-1--blocks-headless) | dump a document's block tree to the terminal | half a day |
| 2 | [Inlines, headless](#milestone-2--inlines-headless) | dump a full AST | half a day |
| 3 | [First window](#milestone-3--first-window) | see a README rendered, unstyled | 2 hours |
| 4 | [Fonts and styled text](#milestone-4--fonts-and-styled-text) | see bold, italic, code, headings | half a day |
| 5 | [The sidebar](#milestone-5--the-sidebar) | click between files | 3 hours |
| 6 | [Navigation](#milestone-6--navigation) | **click a link.** back, forward | 3 hours |
| 7 | [The rest of Markdown](#milestone-7--the-rest-of-markdown) | tables, images, task lists, anchors | half a day |
| 8 | [Polish](#milestone-8--polish) | dark mode, keyboard, missing-file screen | open-ended |
| 9 | [Tests](#milestone-9--tests) | `./run_tests.sh` covers it | 2 hours |

Estimates assume you already know the language, which you do.

---

## Milestone 0 — Make UIKit a package

> **✅ Done.** Recorded here as the description of a repo state that now
> exists, not as work to do.

**Why.** `md-viewer` needs UIKit, and UIKit lived inside
`examples/UI Project/src/UIKit`. Two apps sharing a copied folder is two copies
diverging.

**What was done:**

1. `git mv "examples/UI Project/src/UIKit" examples/uikit/src` — 29 files, with
   history.
2. [`examples/uikit/saule.config`](../../uikit/saule.config):
   ```
   name: "uikit"
   version: "0.1.0"
   kind: "library"
   src_dirs: ["src"]
   min_saule_version: "26.2"
   indent_style: "tab"
   indent_width: 4
   ```
   `src/init.sau` was already the barrel, so `import * from "uikit"` resolves
   with no change to the kit itself. **Not one line of UIKit source was
   edited** — its internal imports (`import * from Framework`, and so on) are
   relative to the importing file, so moving the folder as a unit left them all
   valid.
3. `dependencies: ["../uikit"]` added to
   [`examples/UI Project/saule.config`](../../UI%20Project/saule.config).
4. `import * from UIKit` → `import * from "uikit"` in the seven `UI Project`
   sources that use it: `main`, `Home`, `Shared`, `Test`, `Counter`,
   `Contacts`, `Messenger`.
5. `MaterialIcons-Regular.ttf` and its licence **copied** (not moved) into
   `md-viewer/assets/`. `Assets.resolve` resolves against the *consuming app's*
   `Project.root`, so a font inside the library package would not be findable —
   each app ships its own `assets/` and calls `IconFont.setFontPath` itself.
6. Skips added to `run_examples_diff.sh` for `uikit`, `markdown` and
   `md-viewer` — see [Testing](07-testing.md#the-differential-harness) for why
   that mattered.

**How it was verified:**

| Check | Result |
|---|---|
| `saule check "examples/UI Project"` | no errors (7 files) |
| `saule check examples/uikit` | no errors (28 files) |
| Negative control — one import changed to `"uikitXX"` | 2 errors, so a *failed* resolution is not silent, so the passing check means the real import resolved |
| `saule run "examples/UI Project"` under a 10s watchdog | still running, zero output — the window opened and the frame loop ran |
| `run_examples_diff.sh` | 9 of 14 compared, both engines agree, 5 skipped |

The runtime run is not optional. UIKit's `Modifiers.install(UIKitModifiers())`
executes *on import* of the barrel, and `Framework.sau` throws
`view modifiers are not installed` at build time if it did not — a failure
`saule check` cannot see.

**If you ever want to undo this:** copy the folder back to
`examples/md-viewer/src/UIKit`, drop `"../uikit"` from
[`md-viewer/saule.config`](../saule.config), and use `import * from UIKit`.
Everything downstream is identical.

---

## Milestone 1 — Blocks, headless

**Goal:** `Markdown.parse` handles block structure. No UI, no window, no fonts.

**Files:** `examples/markdown/` — `saule.config` (already written),
`src/Ast.sau`, `src/LineScanner.sau`, `src/BlockParser.sau`, `src/Parser.sau`,
`src/Slugger.sau`, `src/init.sau`.

**Order within the milestone:**

1. `Ast.sau` — all of it, both enums, including the `Inline` variants you will
   not produce yet. Getting the shapes right now saves a rewrite; they are
   [specified here](03-markdown-package.md#the-ast).
2. `LineScanner.sau` — [thirty lines](03-markdown-package.md#linescanner),
   including the CRLF normalization. Do not skip that; this repo has CRLF files
   and the bug it causes (every fence check missing) looks nothing like its
   cause.
3. `Slugger.sau` — [the algorithm](03-markdown-package.md#heading-slugs).
4. `BlockParser.sau` — in this order, testing after each:
   headings → fenced code → thematic break → paragraphs → quotes → lists →
   tables. Emit `Inline.Text(rawLine)` as the only inline for now.
5. `Parser.sau` + `init.sau` — [the facade and barrel](03-markdown-package.md#the-barrel).

**Verify** with a dump script *inside the package*. A library project refuses
`saule run`, but a single **file** runs on its own — and because imports resolve
relative to the importing file's directory, a file sitting beside the parser
sees it with no `dependencies:` entry and no second project to maintain:

```
examples/markdown/
├── saule.config
└── src/
    ├── Ast.sau
    ├── …
    └── dump.sau      ← a `class Main`; `init.sau` does not re-export it
```

`dump.sau` reads a file, parses it, and prints the tree with indentation —
a `match` over `Block` with a depth parameter, ~50 lines. Then:

```bash
saule run examples/markdown/src/dump.sau -- examples/md-viewer/docs/01-scope.md
```

It has to live **inside `src/`**. A `dump.sau` at the package root does not
fall back to `src_dirs` in single-file mode, and every import fails with
`unknown type Block`.

**Done when** the dump of [`01-scope.md`](01-scope.md) shows the right nesting:
headings at the right levels, the big feature table as a `Table`, the bullet
lists as `List` with the right item counts, and the fenced diagram as one
`Code` block. Check `03-markdown-package.md` too — it has nested lists and
tables with pipes in them.

Keep `dump.sau`. It stays useful all the way through
[Milestone 9](#milestone-9--tests).

---

## Milestone 2 — Inlines, headless

**Goal:** paragraph text becomes real `Inline` trees.

**Files:** `src/InlineParser.sau`, `src/Entities.sau`, and the calls into them
from `BlockParser`.

**Order:** literal text and the `pending` buffer first — that alone must not
change the dump's shape. Then, one at a time, each with a case added to your
[edge-case list](03-markdown-package.md#edge-cases-that-will-bite):
escapes → code spans → autolinks → links and images → emphasis → breaks.

**Emphasis is the one that goes wrong.** Write the `snake_case` and `a * b * c`
cases before you write the rule, not after.

**Verify:** the dump now shows `Link(href=…)` nodes. Grep your own output:

```bash
saule run examples/markdown/src/dump.sau -- examples/md-viewer/docs/README.md | grep -c "Link("
```

[`README.md`](README.md) has a known number of links in it. That count is your
first regression test, and it becomes a
[golden file](07-testing.md#golden-files) in Milestone 9.

**Done when** every row of the
[edge-case table](03-markdown-package.md#edge-cases-that-will-bite) produces
what it says it should.

---

## Milestone 3 — First window

**Goal:** a window showing a document. Ugly on purpose — one font, one size,
no bold, no links.

**Files:** `md-viewer/src/main.sau`, `src/render/MarkdownStyle.sau`,
`src/render/DocumentView.sau`, `src/render/BlockRenderer.sau`,
`src/render/InlineRenderer.sau`.

**Do:**

1. `main.sau` — `WindowGroup` + `Theme` + a hardcoded `DocumentView`. Read the
   file with `Io.open` right there in `Main.main` for now; `DocumentStore`
   comes in [Milestone 5](#milestone-5--the-sidebar).
2. `MarkdownStyle.from(theme)` — [the numbers](04-renderer.md#markdownstyle).
3. `InlineRenderer.runs` — handle `Text` and `SoftBreak`; every other variant
   falls through to rendering its children. Plain `Text` views, plain
   `TextStyle`, no faces.
4. `BlockRenderer.render` — the full `match`. `Code` as a `Box` of monospace-ish
   text, `Table` as a stub `Text("[table]")`. The point is that the `match`
   compiles exhaustively now, so later milestones fill in bodies rather than
   restructuring.
5. `DocumentView` — `VStack` + `FlowStack` paragraphs.

**Verify:** `saule run examples/md-viewer` shows this file, scrollable, wrapped
at a readable width.

**Expect to hit** [the claim bug](04-renderer.md#claiming) here — everything
rendered twice. That is the milestone's real lesson and it is better learned
now, with five view types, than in Milestone 7 with twelve.

---

## Milestone 4 — Fonts and styled text

**Goal:** it looks like a document.

**Files:** `src/render/MdTextStyle.sau` (with the `Face` enum),
`src/render/FontRegistry.sau`, `src/render/RunStyle.sau`, and the rewrite of
`InlineRenderer` to carry a `RunStyle`.

**Do:**

1. Put five `.ttf` files in `md-viewer/assets/` and register them in
   `Main.main`. [Which fonts](04-renderer.md#the-fonts-themselves).
2. `FontRegistry` — the `(face, physical size)` cache.
3. `MdTextStyle extends TextStyle`, overriding `applyFont`, `withColor`,
   `withSize`. [The whole trick](04-renderer.md#styled-runs-the-hard-part).
4. `RunStyle` with `bold()` / `italic()` / `struck()` / `linked()` that
   **combine** faces.
5. Thread it through `InlineRenderer.runs`, and split `Inline.Text` per word.
6. Headings in `Face.Bold` at their `MarkdownStyle` sizes.

**Verify:** `**bold**`, `*italic*`, `***both***` and `` `code` `` all look
right in one paragraph, on the same baseline, in this very file. Links show in
`style.link` colour but do nothing yet.

**If bold does not appear:** the font failed to load and you fell back
silently. `FontRegistry` prints once per face — check the terminal. If it
loaded and text still measures wrong, you left `size` nil on the style; see
[trap 1](04-renderer.md#styled-runs-the-hard-part).

---

## Milestone 5 — The sidebar

**Goal:** a file tree you can click.

**Files:** `src/app/Workspace.sau`, `src/app/DocumentStore.sau`,
`src/app/Shell.sau`, `src/app/Sidebar.sau`, `src/app/ContentPane.sau`, and
`main.sau` reduced to a `Shell`.

**Do:**

1. `Workspace.scan` — recursive `Os.list` with the
   [`try/catch`, filtering, sorting and caps](05-app-shell.md#workspace).
2. `DocumentStore` — [load, parse, cache](05-app-shell.md#documentstore).
3. `Shell` — the [three regions and the state slots](05-app-shell.md#shell).
   The router goes in now even though nothing calls `navigate` yet; the sidebar
   uses `router.open`.
4. `Sidebar` — rows, indentation, expand/collapse, current-file highlight.
5. `ContentPane` — the keyed `ScrollView`, and `MissingDocument`.

**Verify:** the app opens on `docs/`, lists all eight files, and clicking each
one renders it. Click a long file, scroll to the middle, click another, click
back to the first: **it must be at the top**, not at the old offset. If it is
not, the `ScrollView` key is missing.

---

## Milestone 6 — Navigation

**Goal:** the feature. Clicking `[Renderer](04-renderer.md)` opens that file.

**Files:** `src/app/Router.sau`, `src/app/RouterScope.sau`,
`src/app/LinkResolver.sau`, `src/render/LinkSpan.sau`, `src/app/TopBar.sau`.

**Do:**

1. `Location` and `Router` — [the full class](05-app-shell.md#router). Wire
   `onChange` to the `Shell`'s `markNeedsBuild`.
2. `RouterScope` — [the ambient lookup](05-app-shell.md#routerscope). Wrap the
   shell's body in it.
3. `LinkResolver.resolve` — [all nine steps](05-app-shell.md#linkresolver).
   Write this one carefully; it is pure and it is where the bugs will be.
4. `LinkSpan` — [the view](04-renderer.md#links), with hover and cursor.
5. `InlineRenderer` emits `LinkSpan` for `Inline.Link`.
6. `TopBar` — back and forward, disabled states honest.

**Verify — this is the acceptance test for the whole project:** open the app on
`docs/`, and from [the index](README.md) click every link in the reading-order
table. Each opens the right file. Back returns. Forward re-advances. Navigate
somewhere new after going back, and forward is correctly dead.

Then click a relative link that crosses a directory (`../saule.config` from a
doc) and confirm it resolves or fails cleanly rather than doing nothing silently.

**If a click does nothing:** check the terminal for
`LinkSpan: no RouterScope above this link`. That message exists precisely
because this is the failure you will hit.

---

## Milestone 7 — The rest of Markdown

**Goal:** the parts stubbed earlier.

- **Tables** — `MdTableView`. A `Grid` of cells, header row in `Face.Bold`,
  column alignment from `ColumnAlign`, a `Divider` under the header, and a
  horizontal `ScrollView` around the lot for wide tables. The tables in
  [Scope](01-scope.md#markdown-the-parser-must-handle) are the test.
- **Anchors** — [the three steps and the frame-ordering trap](05-app-shell.md#anchors).
  Test with `[the AST](02-architecture.md#the-ast)` and with a `#`-only link
  inside one page.
- **Task lists** — a read-only UIKit `Checkbox` plus the item content.
- **Images** — `Graphics.newImage` via UIKit's `Image` view, `src` resolved
  relative to the document. Missing file renders the alt text muted.
- **Code blocks** — `CodeBlockView` proper: background `Box`, rounded, padded,
  horizontal scroll, `softWrap: false`, language label in the corner.

**Verify:** every one of these docs renders with nothing stubbed out and
nothing visibly wrong.

---

## Milestone 8 — Polish

In rough order of value:

1. **Dark mode.** `ThemeData.dark()` in `Shell` state plus a `TopBar` toggle.
   This is also the test that `MarkdownStyle.from(theme)` reads the theme
   instead of hardcoding colours — which it probably does in one or two places.
2. **Keyboard.** [The table](05-app-shell.md#keyboard).
3. **Strikethrough line** — the `ZStack` + `Box` from
   [the renderer](04-renderer.md#styled-runs-the-hard-part).
4. **Link underline** on hover.
5. **Breadcrumb segments clickable.**
6. **A toast** when a link resolves to nothing, instead of a `println`.
   `showToast` is already in UIKit's `Overlay.sau`.
7. **Heading anchor affordance** — a `#` that appears on hover.

---

## Milestone 9 — Tests

See [Testing](07-testing.md) for what to write. In short:

1. Golden files for the parser — input `.md`, expected dump, diffed.
2. Unit fixtures for `LinkResolver` and `Slugger`, which are pure and are where
   the subtle bugs live.
3. Hook both into `run_tests.sh`.
4. Add `examples/md-viewer` to `run_examples_diff.sh` so the VM and the
   interpreter are held to the same output.

---

## The order in one sentence

Parser before pixels, structure before style, one document before many, and
navigation last — because navigation is the only part that cannot be tested
without all the rest of it working.

---

[← The app shell](05-app-shell.md) · [Index](README.md) · [Testing →](07-testing.md)
