[← Index](README.md) · [Architecture →](02-architecture.md)

# 1. Scope

## What v1 is

A desktop window with three regions:

```
┌──────────────────────────────────────────────────────────┐
│  ←  →   docs / 02-architecture.md                        │  TopBar
├──────────────┬───────────────────────────────────────────┤
│ ▸ docs       │  # 2. Architecture                        │
│   README.md  │                                           │
│   01-scope   │  Three projects, one direction of         │
│ ▸ 02-arch…   │  dependency…                              │  ContentPane
│   03-mark…   │                                           │
│              │  ┌─────────────────────────────────┐      │
│  Sidebar     │  │ code block                      │      │
│              │  └─────────────────────────────────┘      │
└──────────────┴───────────────────────────────────────────┘
```

Behaviour, in the order a user meets it:

1. **Opens on a folder.** Default is the app's own `docs/`. A folder can be
   passed as an argument: `saule run examples/md-viewer -- ~/notes`.
2. **Sidebar lists every `.md` file** under that folder, nested by directory,
   sorted, with the current one highlighted.
3. **Clicking a file renders it** in the content pane, scrolled to the top.
4. **Clicking a relative link navigates.** `[Renderer](04-renderer.md)` opens
   that file. `[the AST](#the-ast)` scrolls to that heading.
   `[Router](05-app-shell.md#router)` does both.
5. **Back and forward** work, by button and by keyboard.
6. **External links** (`https://…`) are *not* opened — they render as a
   distinct colour and do nothing on click. Opening a browser is a shell-out to
   the host OS and is not v1's problem.

## Markdown the parser must handle

Enough CommonMark to render these docs and a typical README. Concretely:

| Feature | v1 | Notes |
|---|:--:|---|
| ATX headings `#`–`######` | ✅ | Each gets a [slug](03-markdown-package.md#heading-slugs) for anchors |
| Setext headings (`===` underline) | ✅ | Cheap once the block scanner has lookahead |
| Paragraphs, lazy continuation | ✅ | |
| Fenced code ` ``` ` with info string | ✅ | Language captured, not highlighted |
| Indented code (4 spaces) | ✅ | |
| Block quotes `>` , nested | ✅ | Recursive: a quote holds `table<Block>` |
| Bullet lists `-` `*` `+` | ✅ | Nested by indentation |
| Ordered lists `1.` `1)` | ✅ | Start number preserved |
| Task list items `- [ ]` / `- [x]` | ✅ | Renders a real UIKit `Checkbox`, read-only |
| Thematic break `---` | ✅ | |
| GFM tables | ✅ | With per-column alignment |
| Emphasis `*x*` `_x_` | ✅ | |
| Strong `**x**` `__x__` | ✅ | |
| Strikethrough `~~x~~` | ✅ | |
| Inline code `` `x` `` | ✅ | Backtick-run matching, so `` `` ` `` `` works |
| Links `[t](href "title")` | ✅ | The whole point |
| Images `![alt](src)` | ✅ | Local files only, via `Graphics.newImage` |
| Autolinks `<https://…>` | ✅ | |
| Hard breaks (two spaces, `\`) | ✅ | |
| Backslash escapes | ✅ | |
| HTML entities `&amp;` | ✅ | The named few plus `&#nn;` |

## What v1 deliberately does not do

Each of these is a real cost with a real reason to defer it:

- **Raw HTML blocks and inline HTML.** Passed through as literal text. A viewer
  that renders HTML needs an HTML parser and a second renderer; that is a
  different project.
- **Reference links** (`[t][id]` with `[id]: url` elsewhere). Needs a
  link-reference definition pass before inline parsing. Add it in v2 — the
  block parser already has the right shape for it, see
  [Parser → Two passes](03-markdown-package.md#why-two-passes).
- **Mermaid / syntax highlighting.** Fenced blocks render as monospace text
  with their info string shown as a label. The diagrams in
  [Architecture](02-architecture.md) will show as code in your own viewer —
  that is expected, and rendering them is the best v2 feature on the list.
- **Editing.** Read-only. UIKit has `TextEditor`, so a split-pane editor is
  reachable later; it is not v1.
- **Search.** Deferred with live-reload to v2.
- **Footnotes, definition lists, admonitions.** Not needed by the corpus.

## Non-goals that are worth stating

- **Not a CommonMark conformance project.** Do not chase the spec suite. The
  target is "renders the docs in this repo, and a stranger's README, without
  looking wrong". [Testing](07-testing.md) pins that down with golden files
  rather than a conformance runner.
- **Not a performance project.** These are documents, not game frames. The
  parser runs once per file and the result is [cached](05-app-shell.md#documentstore).
  Reach for a profiler only if a 90 KB file (this repo has several) stutters.

## Definition of done for v1

Running `saule run examples/md-viewer` opens on `docs/`, and every link in
[the index](README.md) navigates to the right file, with back and forward,
including the `#anchor` ones. That is the acceptance test, and it is why the
docs and the app ship together.

---

[← Index](README.md) · [Architecture →](02-architecture.md)
