[← Build order](06-build-order.md) · [Index](README.md)

# 7. Testing

The split that makes this project testable at all is the one from
[Architecture](02-architecture.md#components): **`markdown` imports no UI and
touches no disk.** A parser that is a pure function of a string can be tested
in a terminal; a parser tangled into a view tree can only be tested by looking
at it.

- [What is worth testing](#what-is-worth-testing)
- [Golden files](#golden-files)
- [Unit fixtures](#unit-fixtures)
- [Wiring into run_tests.sh](#wiring-into-run_testssh)
- [The differential harness](#the-differential-harness)
- [What stays manual](#what-stays-manual)

---

## What is worth testing

| Component | How | Why this way |
|---|---|---|
| [`BlockParser`](03-markdown-package.md#blockparser) | [golden files](#golden-files) | the output is a tree; a hand-written assertion per case is unmaintainable |
| [`InlineParser`](03-markdown-package.md#inlineparser) | golden files + [edge-case fixtures](#unit-fixtures) | the edge cases are individually nameable, so they get individual fixtures |
| [`Slugger`](03-markdown-package.md#heading-slugs) | unit fixture | pure, ten lines, collisions are easy to get wrong |
| [`LinkResolver`](05-app-shell.md#linkresolver) | unit fixture | pure, nine branches, and every navigation bug lives here |
| `Workspace.scan` | manual | touches the real filesystem; the interesting failures are permissions and depth |
| Renderer, shell, sidebar | manual | needs a window and eyes |

That table is the honest scope: **the parser and the two pure helpers get real
tests, the UI gets looked at.** Building a headless view-tree assertion harness
for a document viewer would cost more than the bugs it would catch.

---

## Golden files

```
examples/markdown/tests/
├── cases/
│   ├── headings.md          headings.expected
│   ├── lists.md             lists.expected
│   ├── emphasis.md          emphasis.expected
│   ├── code.md              code.expected
│   ├── links.md             links.expected
│   ├── tables.md            tables.expected
│   ├── quotes.md            quotes.expected
│   ├── crlf.md              crlf.expected      ← CRLF in the file itself
│   └── empty.md             empty.expected
└── run.sh
```

The dump format is whatever [`dump.sau`](06-build-order.md#milestone-1--blocks-headless)
prints — one node per line, two spaces per depth:

```
Document title="Headings"
  Heading level=1 slug="headings"
    Text "Headings"
  Paragraph
    Text "Some "
    Strong
      Text "bold"
    Text " text."
```

Stable, diffable, and readable enough that reviewing a changed golden file is
a real review rather than a rubber stamp.

`run.sh` is the whole harness:

```bash
for case in cases/*.md; do
  expected="${case%.md}.expected"
  actual=$("$SAULE_BIN" run ../src/dump.sau -- "$case")
  if ! diff -u "$expected" <(printf '%s\n' "$actual"); then
    failures=$((failures + 1))
  fi
done
```

Regenerate with a `--bless` flag that writes `$actual` over `$expected`. Then
`git diff` shows exactly what your parser change did to every case — which is
the property that makes golden files worth the setup.

**Seed the cases from the
[edge-case table](03-markdown-package.md#edge-cases-that-will-bite)**, and add
a case every time a document renders wrong. That table is a list of tests
someone already wrote down.

`crlf.md` needs actual CRLF bytes and a `.gitattributes` marking it `-text`,
or Git will normalize away the thing it exists to test.

---

## Unit fixtures

`Slugger` and `LinkResolver` are pure, so they fit the repo's existing
`tests/*.sau` convention: a single file that runs, prints, and exits 0.

The catch is that those fixtures are **single-file and dependency-free**, while
these two classes live in projects. Two options:

- **Fixture inside the package.** `examples/markdown/tests/slugger.sau` as its
  own tiny app project, run by `run.sh` alongside the golden files. Keeps the
  code where it belongs.
- **Assertion style:** print `ok` / `FAIL <case>` and exit non-zero on any
  failure, matching what `run_tests.sh` expects. No test framework; a
  `assertEq(actual, expected, name)` helper is fifteen lines and enough.

Cases worth pinning for `LinkResolver`:

| `from` | `href` | expected |
|---|---|---|
| `docs/02-architecture.md` | `04-renderer.md` | `docs/04-renderer.md`, no anchor |
| `docs/02-architecture.md` | `#the-ast` | same path, anchor `the-ast` |
| `docs/02-architecture.md` | `05-app-shell.md#router` | that path, anchor `router` |
| `docs/README.md` | `../saule.config` | one level up |
| `docs/README.md` | `../../markdown/README.md` | two levels up |
| `docs/README.md` | `https://example.com` | `isExternal` true, `resolve` not called |
| `docs/README.md` | `My%20Notes.md` | decoded to `My Notes.md` |
| `docs/README.md` | `nonexistent.md` | `nil` |
| `docs/README.md` | `../markdown/` | `../markdown/README.md` |

For `Slugger`: `## The **AST**` → `the-ast`, `## 2. Architecture` →
`2-architecture`, the same heading twice → `x` then `x-1`, and a heading with
an accented character keeping it rather than vanishing.

---

## Wiring into run_tests.sh

`run_tests.sh` runs `tests/*.sau` through the debug build and requires exit 0,
with `SAULE_DIFF=1` additionally requiring identical output from both engines.
The Markdown suites do not fit that per-file shape, so add them as a block that
runs `examples/markdown/tests/run.sh` and folds its exit status into
`failures`.

Respect the script's existing conventions when you do: honour `SAULE_BIN`, and
make `run.sh` exit non-zero on any mismatch so CI can gate on it.

---

## The differential harness

`run_examples_diff.sh` runs every example project under **both** the VM and the
tree-walking interpreter and requires identical output. Its own header lists
what it has caught that fixtures did not — a `match` guard firing on a
non-matching pattern, a cross-module `self.super()` recursing forever, an enum
variant's `.value` reading `nil`.

A recursive-descent parser built on
[enums with payloads and nested `match`](03-markdown-package.md#the-ast),
running across seven modules, is close to the ideal input for that harness.

**Add `dump.sau`** — it is a terminating CLI that exercises the whole parser.
Give it a fixed default input so a bare
`saule run examples/markdown/src/dump.sau` produces deterministic output with
no arguments.

**Skips for the other three are already in `skip_reason()`** — added in
[Milestone 0](06-build-order.md#milestone-0--make-uikit-a-package):

```bash
*/uikit)     echo "library: no entry point to run" ;;
*/markdown)  echo "library: no entry point to run" ;;
*/md-viewer) echo "interactive: opens a window and loops until closed" ;;
```

They were not optional. `uikit` and `markdown` are `kind: "library"`, so
`saule run` refuses them — **identically under both engines**, which the
harness would have counted as two more projects agreeing. `md-viewer` will hang
on its window once it exists, and errors identically today. All three would
have been green rows testing nothing, which is the precise failure the
harness's own header warns about. An exclusion it prints beats one it hides.

Current state: `9 of 14 projects compared, 5 skipped, both engines agree`.

---

## What stays manual

Keep a checklist next to the app and walk it before calling a milestone done:

- [ ] Opens on `docs/` with no arguments, from any working directory
- [ ] Every link in [the index's reading-order table](README.md#read-in-this-order) navigates
- [ ] Back and forward, including after branching away from a back
- [ ] `#anchor` links land on the heading, both cross-file and same-file
- [ ] Scroll position resets on document change, survives an anchor jump
- [ ] Bold, italic, bold-italic and inline code render in the right faces
- [ ] Wide code blocks scroll horizontally instead of wrapping
- [ ] Tables render with the right alignment
- [ ] Light and dark both readable
- [ ] A missing font degrades to the default face, no crash
- [ ] A broken link shows something, does nothing silently
- [ ] Pointed at a folder with no Markdown, says so
- [ ] Pointed at a huge folder, does not hang

The first two items are the [definition of done](01-scope.md#definition-of-done-for-v1),
and they are the reason these docs live inside the app they describe.

---

[← Build order](06-build-order.md) · [Index](README.md)
