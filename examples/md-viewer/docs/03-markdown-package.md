[← Architecture](02-architecture.md) · [Index](README.md) · [The renderer →](04-renderer.md)

# 3. The `markdown` package

A library project with no UI and no filesystem access. In goes a string, out
comes a [`Document`](02-architecture.md#the-ast).

```
examples/markdown/
├── saule.config          name: "markdown", kind: "library"
└── src/
    ├── init.sau          the barrel
    ├── Ast.sau
    ├── LineScanner.sau
    ├── BlockParser.sau
    ├── InlineParser.sau
    ├── Entities.sau
    ├── Slugger.sau
    └── Parser.sau
```

- [Why two passes](#why-two-passes)
- [The AST](#the-ast)
- [LineScanner](#linescanner)
- [BlockParser](#blockparser)
- [InlineParser](#inlineparser)
- [Heading slugs](#heading-slugs)
- [Entities and escapes](#entities-and-escapes)
- [The barrel](#the-barrel)
- [Edge cases that will bite](#edge-cases-that-will-bite)

---

## Why two passes

Markdown is not context-free and cannot be parsed with one scan. `*` means
emphasis inside a paragraph and nothing at all inside a fenced code block, and
you cannot know which until you know what block you are in.

So: **blocks first, inlines second.**

1. [`BlockParser`](#blockparser) walks lines and builds the block structure,
   collecting each paragraph's raw text without looking at it.
2. [`InlineParser`](#inlineparser) then runs over that raw text, per block.

This is also where a v2 [reference-link](01-scope.md#what-v1-deliberately-does-not-do)
pass would slot in — between the two, stripping `[id]: url` lines into a map
the inline pass consults.

Saule has **no pattern language**: `String.find` and `String.replace` work on
literal text. That rules out a regex-driven parser and pushes you toward a
character cursor, which is the right implementation for Markdown regardless.

---

## The AST

`src/Ast.sau`. Enums carry payloads; `match` in the
[renderer](04-renderer.md) binds them typed, and the typechecker
[requires exhaustiveness](02-architecture.md#the-ast).

```saule
export enum ColumnAlign
    Left,
    Center,
    Right
end

export enum Inline
    Text(value: string),
    Code(value: string),
    Emph(children: table<Inline>),
    Strong(children: table<Inline>),
    Strike(children: table<Inline>),
    Link(href: string, title: string?, children: table<Inline>),
    Image(src: string, alt: string, title: string?),
    SoftBreak,
    HardBreak
end

export enum Block
    Heading(level: integer, slug: string, children: table<Inline>),
    Paragraph(children: table<Inline>),
    Code(language: string?, source: string),
    Quote(children: table<Block>),
    List(ordered: boolean, start: integer, tight: boolean, items: table<ListItem>),
    Table(align: table<ColumnAlign>, head: table<TableCell>, rows: table<table<TableCell>>),
    Rule
end

export class ListItem
    -- nil means "not a task item"; false is an unchecked box.
    checked: boolean?
    blocks: table<Block>

    fn init(blocks: table<Block> = {}, checked: boolean? = nil)
        self.blocks = blocks
        self.checked = checked
    end
end

export class TableCell
    children: table<Inline>

    fn init(children: table<Inline> = {})
        self.children = children
    end
end

export class HeadingRef
    level: integer
    slug: string
    text: string

    fn init(level: integer, slug: string, text: string)
        self.level = level
        self.slug = slug
        self.text = text
    end
end

export class Document
    blocks: table<Block>

    -- The first H1, if there is one. The sidebar prefers it over the filename.
    title: string?

    -- Every heading in order, so `#anchor` navigation and a future table of
    -- contents both have one source of truth.
    headings: table<HeadingRef>

    fn init(blocks: table<Block>, title: string?, headings: table<HeadingRef>)
        self.blocks = blocks
        self.title = title
        self.headings = headings
    end
end
```

Three notes on the shapes:

- **`Heading` carries its slug.** Computing it at parse time means the renderer
  and the router agree by construction, instead of both re-deriving it and
  disagreeing on the day someone writes a heading with a colon in it.
- **`List.tight`** is CommonMark's tight/loose distinction: a list whose items
  are separated by blank lines gets paragraph spacing, a tight one does not.
  Cheap to record here, ugly to guess later in the renderer.
- **`TableCell` is a class, not a bare `table<Inline>`.** A
  `table<table<table<Inline>>>` for the rows is technically the same thing and
  unreadable at the call site.

---

## LineScanner

`src/LineScanner.sau`. A cursor over lines, with the one-line lookahead that
setext headings and table delimiter rows both need.

```saule
export class LineScanner
    local lines: table<string>
    local pos: integer

    fn init(source: string)
        -- Normalize line endings first: this repo has CRLF files, and a
        -- trailing \r turns every "```" check into a miss.
        local normalized: string = String.replace(source, "\r\n", "\n")
        normalized = String.replace(normalized, "\r", "\n")

        self.lines = String.split(normalized, "\n")
        self.pos = 1
    end

    fn eof() -> boolean
        return self.pos > #self.lines
    end

    fn peek(offset: integer = 0) -> string?
        local at: integer = self.pos + offset

        if at < 1 or at > #self.lines then
            return nil
        end

        return self.lines[at]
    end

    fn next() -> string?
        local line: string? = self.peek()

        if line != nil then
            self.pos = self.pos + 1
        end

        return line
    end

    fn skipBlank() -> integer
        local skipped: integer = 0

        while not self.eof() and String.trim(self.lines[self.pos]) == "" do
            self.pos = self.pos + 1
            skipped = skipped + 1
        end

        return skipped
    end

    -- Rewind by one. Block starters peek, decide, and sometimes hand the line
    -- back to the paragraph rule.
    fn back() -> nil
        if self.pos > 1 then
            self.pos = self.pos - 1
        end
    end
end
```

`String.split(s, "\n")` always returns `occurrences + 1` pieces, so a file
ending in a newline yields a trailing empty line. That is correct and harmless —
`skipBlank` eats it.

---

## BlockParser

`src/BlockParser.sau`. One loop, one dispatch, one fallback.

```saule
export class BlockParser
    local scanner: LineScanner
    local slugger: Slugger
    local headings: table<HeadingRef>

    fn run() -> table<Block>
        local out: table<Block> = {}

        while not self.eof() do
            self.scanner.skipBlank()

            if self.scanner.eof() then
                break
            end

            local block: Block? = self.tryStarters()

            if block == nil then
                Table.insert(out, self.paragraph())
            else
                Table.insert(out, block!)
            end
        end

        return out
    end
end
```

`tryStarters` asks each rule in turn and takes the first that claims the line.
**Order matters** — these are the collisions:

| Order | Rule | Beats | Because |
|---|---|---|---|
| 1 | fenced code ` ``` ` / `~~~` | everything | inside a fence nothing else is markup |
| 2 | thematic break `---` `***` `___` | setext, list | `---` under a paragraph is a setext H2, alone it is a rule |
| 3 | ATX heading `#` | paragraph | |
| 4 | block quote `>` | paragraph | |
| 5 | list `-` `*` `+` `1.` | paragraph | after rule, so `***` is not a bullet |
| 6 | table (row + delimiter row on next line) | paragraph | needs the [lookahead](#linescanner) |
| 7 | indented code (4 spaces) | paragraph | **not** inside a list item, where 4 spaces is continuation |
| 8 | paragraph | — | the fallback; also handles setext |

### The rules, in prose

**Fenced code.** Opening fence is 3+ backticks or 3+ tildes; the info string is
everything after them, trimmed, first word taken as the language. Consume lines
verbatim until a closing fence of *the same character and at least the same
length*. **End of input closes an unterminated fence** — do not throw. A README
with a stray fence should still render.

**ATX heading.** 1–6 `#`, then a required space, then text. Strip an optional
trailing run of `#`. Feed the text to [`InlineParser`](#inlineparser) and to
[`Slugger`](#heading-slugs), and record a `HeadingRef`.

**Setext heading.** Handled inside `paragraph`: after collecting the first
line, if the next line is all `=` it is an H1, all `-` it is an H2. This is why
the thematic-break rule runs *before* list but the setext check lives in
`paragraph` — `---` after text is a heading, `---` after a blank line is a
rule.

**Block quote.** Strip one leading `>` and at most one following space from
each line in the run, then **recurse**: `BlockParser(strippedText).run()`
returns the inner `table<Block>`. Lazy continuation — an unmarked line
continuing a quote paragraph — is a v1.1 nicety; without it a wrapped quote
just splits into two quotes, which looks nearly the same.

**Lists.** The fiddly one, and the only place worth planning carefully:

1. Detect a marker: `-`/`*`/`+` plus space, or digits plus `.`/`)` plus space.
2. `ordered` and `start` come from the first item.
3. The item's **content indent** is the column after the marker and its space.
   Every following line indented at least that far belongs to this item.
4. Collect the item's lines, strip the content indent, and **recurse** — an
   item's body is `table<Block>`, so nested lists, code blocks and paragraphs
   inside items all come free.
5. A blank line between items sets `tight = false`.
6. A line at lower indentation, or a different marker type, ends the list.
7. Immediately after step 1, check for `[ ] ` or `[x] ` and set `checked`.

Bullets change type mid-list (`-` then `*`) in real documents. CommonMark says
that starts a new list; matching that is fine and simpler than the alternative.

**Tables.** A line containing `|`, whose *next* line is a delimiter row
(`|---|:--:|---:|`), starts a table. Split on `|`, drop empty leading and
trailing cells from the pipe-wrapped form, read alignment from the colons, then
parse every cell through `InlineParser`. Rows with the wrong cell count are
padded or truncated to the header's width rather than rejected.

**Paragraph.** Collect lines until a blank line or until any other starter
claims the next line. Join with `"\n"` and hand the lot to `InlineParser`,
which turns the newlines into `Inline.SoftBreak`.

---

## InlineParser

`src/InlineParser.sau`. A character cursor over one block's raw text.
1-based indices, `String.sub` for slicing, `String.byte` when you need to
compare a character cheaply.

```saule
export class InlineParser
    local src: string
    local pos: integer
    local pending: string      -- literal text accumulated but not yet flushed

    fn run() -> table<Inline>
        local out: table<Inline> = {}
        local length: integer = String.len(self.src)

        while self.pos <= length do
            local ch: string = String.sub(self.src, self.pos, self.pos)
            local node: Inline? = nil

            if ch == "\\" then
                node = self.escape()
            else
                if ch == "`" then
                    node = self.codeSpan()
                else
                    if ch == "<" then
                        node = self.autolink()
                    else
                        if ch == "!" or ch == "[" then
                            node = self.link()
                        else
                            if ch == "*" or ch == "_" or ch == "~" then
                                node = self.emphasis()
                            else
                                if ch == "\n" then
                                    node = self.lineBreak()
                                end
                            end
                        end
                    end
                end
            end

            if node == nil then
                -- Nothing claimed it: it is literal text.
                self.pending = self.pending .. ch
                self.pos = self.pos + 1
            else
                self.flush(out)
                Table.insert(out, node!)
            end
        end

        self.flush(out)

        return out
    end
end
```

> That `if` ladder is what the code actually looks like, and it is ugly. If
> you would rather read a `match`, dispatch on `String.byte` instead — integer
> literal patterns make it a flat `match` with a `case _` fallback, and it is
> faster besides. Worth doing on the second pass, not the first.

**The `pending` buffer is the core trick.** Literal characters accumulate into
a string; every time a real node is produced, `flush` emits the accumulated
text as one `Inline.Text` and clears it. Without it you get one `Inline.Text`
per character and a renderer that lays out a hundred views per sentence.

### Each rule

**Escape.** `\` followed by an ASCII punctuation character emits that character
as literal text. `\` before anything else is a literal backslash. `\` at end of
line is a `HardBreak`.

**Code span.** Count the opening backtick run, then search for the *next run of
exactly the same length*. No match means the backticks are literal. Strip one
leading and one trailing space when both are present and the content is not all
spaces. **Nothing inside a code span is markup** — this is why it is checked
before emphasis and links.

**Autolink.** `<` … `>` where the contents look like `scheme://…` or an email.
Emits `Inline.Link` with the text as its own label. Anything else beginning
with `<` is literal text in v1 — that is the
[no-HTML decision](01-scope.md#what-v1-deliberately-does-not-do).

**Link and image.** `[` starts a link, `![` an image. Scan forward for the
matching `]`, **counting nesting** so `[a [b] c](url)` works. Then require `(`,
read the destination up to whitespace or `)`, optionally read a `"title"`, then
require `)`. If any of that fails, back all the way up and treat the `[` as
literal text — a bare `[TODO]` in a document must not eat the rest of the
paragraph. The label is parsed by a **nested `InlineParser`**, so
`[**bold** link](x.md)` works. Angle-bracketed destinations `(<a b.md>)` allow
spaces in filenames; support them, `docs/My Notes.md` exists in the wild.

**Emphasis.** Full CommonMark delimiter runs are a rabbit hole. Implement the
90% rule:

- Count the run of `*` or `_` at the cursor. A run of 2+ tries `Strong` first,
  a run of 1 tries `Emph`, `~~` tries `Strike`.
- Search forward for a closing run of the same character and length.
- The opener must be followed by a non-space and the closer preceded by a
  non-space, otherwise `a * b * c` becomes emphasis.
- For `_`, additionally require that the delimiter is not surrounded by
  alphanumerics, so `snake_case_name` stays intact. This one matters — it fires
  constantly in a repo full of identifiers.
- No closer found → literal text.
- Contents recurse through a nested `InlineParser`.

**Line break.** Two or more trailing spaces before `\n` → `HardBreak`. A bare
`\n` → `SoftBreak`. The renderer decides what a soft break means (it rewraps,
so: a space).

---

## Heading slugs

`src/Slugger.sau`. GitHub's algorithm, because that is what everyone's links
already assume:

1. Take the heading's **rendered text** — inline markup stripped, so
   `## The **AST**` slugs from `The AST`.
2. Lowercase (`String.lower`).
3. Remove everything that is not alphanumeric, space, hyphen or underscore.
4. Spaces → hyphens.
5. On collision, append `-1`, `-2`, … — hence the `seen` table, and hence
   `Slugger` being an *instance* per document rather than a static function.

`## 2. Architecture` → `2-architecture`. Check that against the anchors in
[the architecture doc](02-architecture.md#components) — those links are the
test.

Non-ASCII needs a rule, and the obvious one is wrong. "Keep every character
whose `String.byte` is above 127" saves accented letters — but it also keeps
**punctuation**, and an em dash in a heading then produces a slug nobody links
to. `## Milestone 1 — Blocks, headless` must slug to `milestone-1--blocks-headless`
(dash gone, its two spaces collapsing into the doubled hyphen), which is what
every link in [Build order](06-build-order.md) is written against. Keeping the
dash gives `milestone-1-—-blocks-headless` and every one of those links breaks.

So step 3 splits by *category*, not by codepoint:

- **Keep** non-ASCII **letters and digits** — `Über`, `naïve`, `日本語`.
- **Drop** non-ASCII **punctuation and symbols** — `—`, `–`, `…`, `“”`, `«»`.

Saule has no Unicode character-class table, so approximate it: drop the dozen
punctuation codepoints that actually appear in prose (`—` 8212, `–` 8211,
`…` 8230, the curly quotes 8216–8221, `«»` 171/187, `·` 183, `•` 8226) and keep
everything else above 127. A `table` of banned codepoints checked with
`String.byte` is ten lines and covers every document you will meet. It will not
match GitHub on Cyrillic or CJK — GitHub keeps those too, so it will — and it
will not collide.

This is the kind of rule that is invisible until every anchor link in a doc set
breaks at once. [Test it](07-testing.md#unit-fixtures) with a heading
containing an em dash and a heading containing an accented letter.

---

## Entities and escapes

`src/Entities.sau`. A small map — `&amp; &lt; &gt; &quot; &#39; &nbsp; &mdash;
&ndash; &hellip; &copy;` — plus numeric `&#nn;` and `&#xhh;` via
`String.char`. Applied to literal text only, never inside code spans or fenced
code. An unrecognised entity is left alone.

---

## The barrel

`src/init.sau` is the package's public surface —
[folder-module rules](../../../README.md#folder-modules-initsau) apply, and
re-export only happens in an `init.sau`:

```saule
import * from Ast
import * from Parser
```

`LineScanner`, `BlockParser`, `InlineParser`, `Slugger` and `Entities` stay
internal. Consumers get the AST and the facade:

```saule
export class Markdown
    static fn parse(source: string) -> Document
        local slugger: Slugger = Slugger()
        local parser: BlockParser = BlockParser(LineScanner(source), slugger)
        local blocks: table<Block> = parser.run()

        return Document(blocks, parser.title(), parser.headings())
    end

    -- Exposed because the renderer's tests want it, and because a one-line
    -- string is a legitimate thing to parse.
    static fn parseInline(text: string) -> table<Inline>
        return InlineParser(text).run()
    end
end
```

Then in the app: `import Markdown from "markdown"`, because `markdown` is
listed in [md-viewer's `dependencies`](../saule.config).

---

## Edge cases that will bite

Collect these as [golden files](07-testing.md#golden-files) as you hit them.

| Input | Must produce |
|---|---|
| `snake_case_identifier` | literal text, no emphasis |
| `a * b * c` | literal text |
| `**bold** and *italic*` | two separate emphasis nodes |
| `` `` ` `` `` | a code span containing one backtick |
| `[not a link` | literal text, paragraph survives |
| `[a [b] c](x.md)` | one link, label `a [b] c` |
| `![alt](img.png)` | image, not a link labelled `!` |
| unterminated ` ``` ` | code block to end of file |
| `---` after a paragraph line | setext H2 |
| `---` after a blank line | thematic break |
| `- [x] done` | list item with `checked = true` |
| CRLF file | identical AST to the LF version |
| empty file | `Document` with zero blocks, no crash |
| a heading twice | slugs `x` and `x-1` |
| `\|` inside a table cell | one cell, literal pipe |

---

[← Architecture](02-architecture.md) · [Index](README.md) · [The renderer →](04-renderer.md)
