[← The markdown package](03-markdown-package.md) · [Index](README.md) · [The app shell →](05-app-shell.md)

# 4. The renderer

`md-viewer/src/render/`. The only layer that imports both
[`markdown`](03-markdown-package.md) and `uikit`. In goes a
[`Document`](02-architecture.md#the-ast), out comes a UIKit `View`.

- [Shape of the code](#shape-of-the-code)
- [Styled runs: the hard part](#styled-runs-the-hard-part)
- [FontRegistry](#fontregistry)
- [InlineRenderer](#inlinerenderer)
- [BlockRenderer](#blockrenderer)
- [MarkdownStyle](#markdownstyle)
- [Claiming](#claiming)
- [Links](#links)
- [Block-by-block mapping](#block-by-block-mapping)

---

## Shape of the code

Two recursive **static functions**, plus a handful of real views for the things
that need state or custom layout.

```saule
export class BlockRenderer
    static fn renderAll(blocks: table<Block>, style: MarkdownStyle) -> table<View>
    static fn render(block: Block, style: MarkdownStyle) -> View
end

export class InlineRenderer
    -- One `View` per run of text. The caller drops them into a FlowStack.
    static fn runs(nodes: table<Inline>, style: MarkdownStyle, run: RunStyle) -> table<View>
end
```

They are functions rather than views on purpose. A `View` per AST node puts a
`body` — and therefore an `Element`, a rebuild and a
[claim](#claiming) — between every `Strong` and its text. For a leaf that draws
three words, all of that is overhead you would then have to reason about. The
things that *are* views are the ones that need element state: the
[code block](#block-by-block-mapping) (it scrolls), the
[table](#block-by-block-mapping) (it lays out columns), and the
[link span](#links) (it hit-tests and hovers).

`DocumentView` is the one view at the top:

```saule
export class DocumentView extends View
    document: Document

    fn init(document: Document, key: string? = nil)
        self.super(key)
        self.document = document
    end

    fn body(context: BuildContext) -> View
        local style: MarkdownStyle = MarkdownStyle.from(Theme.of(context))

        return VStack(alignment: StackAlignment.Leading, spacing: style.blockGap) do
            for child: View in BlockRenderer.renderAll(self.document.blocks, style) do
                show(child)
            end
        end
    end
end
```

`show(child)` is UIKit's re-emit helper: a view built *outside* the open
content block has to be announced to it. See [Claiming](#claiming).

---

## Styled runs: the hard part

**The problem.** UIKit's `TextStyle` carries exactly two things: a `Color` and
a `float` size. Bold, italic and monospace are not sizes — they are different
*faces*, loaded from different `.ttf` files. So there is no way to spell
"bold" with the toolkit as it ships.

**The wrong fix** is a custom `View` that measures and draws text itself. It
duplicates `Text`'s wrapping, caching and alignment, all of which already work.

**The right fix** is one subclass, because of how `Text` is written: every path
in `Text.layout` and `Text.paint` goes through `style.measure`,
`style.lineHeight`, `style.draw` and `style.apply` — and *all four* of those
call `self.applyFont()`. `applyFont` is public and virtual. Override it and
UIKit's own `Text` view renders in any face you like:

```saule
export enum Face
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono
end

export class MdTextStyle extends TextStyle
    face: Face

    fn init(color: Color? = nil, size: float? = nil, face: Face = Face.Regular)
        self.super(color, size)
        self.face = face
    end

    -- The one override that makes bold possible. `TextStyle.applyFont` binds
    -- the default face at a physical size; this binds *our* face at the same
    -- physical size, and falls back to the parent when the file is missing so
    -- a stripped checkout still renders.
    fn applyFont() -> nil
        local size: float? = self.size

        if size == nil then
            self.super.applyFont()

            return
        end

        local handle: integer? = FontRegistry.handle(self.face, size! * Display.scale())

        if handle == nil then
            self.super.applyFont()

            return
        end

        Graphics.setFont(handle!)
    end

    -- `TextStyle.withColor` returns a `TextStyle`, which would silently drop
    -- the face the moment anything chained a modifier. An override may return
    -- a subclass, so these keep it.
    fn withColor(color: Color) -> MdTextStyle
        return MdTextStyle(color, self.size, self.face)
    end

    fn withSize(size: float) -> MdTextStyle
        return MdTextStyle(self.color, size, self.face)
    end

    fn withFace(face: Face) -> MdTextStyle
        return MdTextStyle(self.color, self.size, face)
    end
end
```

Two traps this closes:

1. **Never leave `size` nil on a styled run.** `TextStyle.fontScale` returns
   `1.0` for a nil size and `Display.scale()` otherwise, and `measure` divides
   by it. A nil size means the default face at whatever the transform does —
   which measures wrong the moment you mix it with a sized run on the same line.
   Always give `MdTextStyle` a size, even the body size.
2. **Do not use `Text.font()` / `.foregroundStyle()` on styled runs.** They
   call `withSize` / `withColor` on the style — which is why those overrides
   exist above. With them, the modifiers are safe. Without them, the face
   vanishes and only some of your bold text is bold, which is a maddening bug
   to chase. Construct with `Text(data, style)` and let the style carry
   everything.

**Strikethrough** has no font face. Draw it: wrap the run's `Text` in a
`ZStack` with a 1px `Box` at the vertical centre, or — simpler — accept a
`Colors.textMuted` rendering in v1 and do the line in
[Milestone 8](06-build-order.md#milestone-8--polish).

---

## FontRegistry

`render/FontRegistry.sau`. One static cache, keyed by `(face, physical size)`,
because a font handle is a rasterised atlas — asking for one per frame rebuilds
it per frame. This is exactly what UIKit's `IconFont` does for Material Icons;
copy its shape.

```saule
export class FontRegistry
    static local paths: table = {}
    static local handles: table = {}
    static local warned: table = {}

    -- Point a face at a `.ttf`. Called once at startup from `main.sau`.
    static fn register(face: Face, path: string) -> nil
        paths[face.value] = Assets.resolve(path)
        handles = {}
    end

    -- A handle for this face at this *physical* size, or nil to fall back.
    static fn handle(face: Face, physical: float) -> integer?
        local path = paths[face.value]

        if path == nil then
            return nil
        end

        local key: string = face.value .. "@" .. String.format("%.1f", physical)
        local cached = handles[key]

        if cached != nil then
            return cached as integer
        end

        local created: integer? = Graphics.loadFont(physical, path as string)

        if created == nil then
            if warned[face.value] == nil then
                warned[face.value] = true
                println("FontRegistry: cannot load " .. (path as string))
            end

            return nil
        end

        handles[key] = created!

        return created
    end
end
```

`Graphics.loadFont` reports failure as `nil` rather than raising — that is the
one to use, not `Graphics.newFont`, so a missing font degrades to the default
face instead of ending the program.

Make `Face` a **valued enum** (`Regular = "regular"`, …) so `face.value` is a
usable table key.

### The fonts themselves

Five files in `md-viewer/assets/`, plus the icon font:

| Face | Suggested file | Used by |
|---|---|---|
| Regular | `Inter-Regular.ttf` or any UI sans | body text |
| Bold | `Inter-Bold.ttf` | `Strong`, headings |
| Italic | `Inter-Italic.ttf` | `Emph` |
| BoldItalic | `Inter-BoldItalic.ttf` | nested `Strong`+`Emph` |
| Mono | `JetBrainsMono-Regular.ttf` | `Inline.Code`, `Block.Code` |
| Icons | `MaterialIcons-Regular.ttf` | copy from `examples/UI Project/assets/` |

All paths go through `Assets.resolve`, which makes them absolute against
`Project.root` — a bare relative path resolves against the *shell's* working
directory, so `saule run examples/md-viewer` from the repo root would find
nothing. Point the icon font at yours too:
`IconFont.setFontPath("assets/MaterialIcons-Regular.ttf")`.

Pick fonts with a licence that allows redistribution (SIL OFL: Inter, JetBrains
Mono, Source Sans/Code Pro all qualify). If you would rather not vendor
anything, register nothing — every face falls back to the engine default and
the app still runs, just without bold.

---

## InlineRenderer

Turns `table<Inline>` into a `table<View>` of per-word runs, which the caller
drops into a **`FlowStack`** — the UIKit layout that wraps to width
(`Layout.sau`). That is what makes `a **bold** word` flow as one paragraph
instead of three stacked blocks.

```saule
export class InlineRenderer
    static fn runs(nodes: table<Inline>, style: MarkdownStyle, run: RunStyle) -> table<View>
        local out: table<View> = {}

        for node: Inline in nodes do
            match node
                case Inline.Text(value) then
                    InlineRenderer.words(value, run, style, out)

                case Inline.Code(value) then
                    Table.insert(out, InlineRenderer.codeRun(value, run, style))

                case Inline.Emph(children) then
                    InlineRenderer.append(out, InlineRenderer.runs(children, style, run.italic()))

                case Inline.Strong(children) then
                    InlineRenderer.append(out, InlineRenderer.runs(children, style, run.bold()))

                case Inline.Strike(children) then
                    InlineRenderer.append(out, InlineRenderer.runs(children, style, run.struck()))

                case Inline.Link(href, title, children) then
                    local inner: table<View> = InlineRenderer.runs(children, style, run.linked(style))
                    Table.insert(out, LinkSpan(href: href, tooltip: title, children: inner))

                case Inline.Image(src, alt, title) then
                    Table.insert(out, InlineRenderer.image(src, alt, style))

                case Inline.SoftBreak then
                    -- A soft break is a space: FlowStack rewraps anyway.
                    Table.insert(out, Frame(width: style.spaceWidth, height: 0.0))

                case Inline.HardBreak then
                    -- Force the flow onto a new line by filling the rest of it.
                    Table.insert(out, Expand(child: Frame(height: 0.0)))
            end
        end

        return out
    end
end
```

**One view per word, not per run.** `words()` splits an `Inline.Text` on spaces
and emits a `Text` per word, because `FlowStack` breaks *between children* —
a whole sentence as one child is one unbreakable block. The cost is a view per
word; a 90 KB document is a few thousand of them, which is fine for a document
and would not be for a game loop.

`HardBreak` is the awkward one: `FlowStack` has no explicit break. `Expand`
inside a flow run is the trick — it eats the remainder of the line. If that
misbehaves, the fallback is to have `BlockRenderer.paragraph` split the inline
list on `HardBreak` and emit one `FlowStack` per hard-broken line inside a
`VStack`. That is more code and more obviously correct; switch to it the moment
the `Expand` version surprises you.

### RunStyle

A tiny immutable value carried down the recursion — the renderer's equivalent
of an inherited text context.

```saule
export class RunStyle
    face: Face
    color: Color
    size: float
    strike: boolean
    inLink: boolean

    fn bold() -> RunStyle          -- Regular→Bold, Italic→BoldItalic
    fn italic() -> RunStyle        -- Regular→Italic, Bold→BoldItalic
    fn struck() -> RunStyle
    fn linked(style: MarkdownStyle) -> RunStyle
    fn textStyle() -> MdTextStyle  -- MdTextStyle(self.color, self.size, self.face)
end
```

`bold()` and `italic()` **combine** rather than replace — that is the whole
reason `Face` has a `BoldItalic` variant. `***both***` parses as `Strong` around
`Emph`, and each level narrows the face.

---

## BlockRenderer

A `match` over `Block`, and the exhaustiveness check is doing real work here:
add a variant to the enum and this function stops compiling until you handle it.

```saule
static fn render(block: Block, style: MarkdownStyle) -> View
    return match block
        case Block.Heading(level, slug, children) then
            BlockRenderer.heading(level, slug, children, style)

        case Block.Paragraph(children) then
            FlowStack(
                children: InlineRenderer.runs(children, style, style.bodyRun()),
                spacing: style.spaceWidth,
                runSpacing: style.lineGap,
                alignment: StackAlignment.Leading
            )

        case Block.Code(source, language) then
            CodeBlockView(language: language, source: source)

        case Block.Quote(children) then
            HStack(sizing: StackSize.Fill, spacing: 12.0) do
                Box(color: style.quoteBar, width: 3.0).expanded()
                VStack(alignment: StackAlignment.Leading, spacing: style.blockGap) do
                    for child: View in BlockRenderer.renderAll(children, style) do
                        show(child)
                    end
                end.expanded()
            end

        case Block.List(ordered, start, tight, items) then
            BlockRenderer.list(ordered, start, tight, items, style)

        case Block.Table(align, head, rows) then
            MdTableView(align: align, head: head, rows: rows)

        case Block.Rule then
            Divider().padding(vertical: style.blockGap)
    end
end
```

`Quote` and list items recurse straight back into `renderAll`, which is why
nested quotes and lists-inside-lists need no extra code.

---

## MarkdownStyle

Every size, gap and colour in one place, derived from the ambient theme so
light and dark both work:

```saule
export class MarkdownStyle
    h1: float   h2: float   h3: float
    h4: float   h5: float   h6: float
    body: float
    code: float
    blockGap: float
    lineGap: float
    spaceWidth: float
    text: Color
    muted: Color
    link: Color
    linkExternal: Color
    codeBg: Color
    quoteBar: Color
    ruleColor: Color

    static fn from(theme: ThemeData) -> MarkdownStyle
end
```

Suggested scale, for a 15px body: h1 30, h2 24, h3 19, h4 16, h5 15, h6 14 —
h5 and h6 differentiated by weight and colour rather than size, which is what
they are for. `blockGap` 14, `lineGap` 4, `spaceWidth` 4.5.

Build it fresh in `DocumentView.body` from `Theme.of(context)`. It is a dozen
float assignments; caching it would be optimising the wrong thing and would
break theme switching.

---

## Claiming

The one UIKit rule that will cost you an afternoon if you skip it. From the
toolkit's own README:

> every `View`-typed field in this kit is assigned through `claimed` /
> `claimedAll`

Because `View.init` announces every constructed view to the innermost open
content block, a view you build and then *store in a field* has already been
recorded as a loose sibling. It must be claimed back, or it renders twice —
once where you put it, once floating where it was built.

Rules for the renderer:

- Views built inside a `do … end` content block and left as statements: fine,
  nothing to do.
- Views built **outside** a block and passed in — which is exactly what
  `BlockRenderer.renderAll` returns — go in via `children:` (the constructors
  route it through `childrenOf` / `claimedAll`) or via `show(child)` inside the
  block.
- A custom view of yours storing a `View` field assigns it through
  `claimed(...)`, and a `table<View>` field through `claimedAll(...)`.

`CodeBlockView`, `MdTableView` and `LinkSpan` all hold children — all three
need this.

**Symptom to recognise:** every paragraph appears twice, or once correctly and
once stacked at the top of the document. That is a missing claim, not a layout
bug.

---

## Links

`render/LinkSpan.sau` — the view the whole app exists to make work.

```saule
export class LinkSpan extends View
    href: string
    tooltip: string?
    children: table<View>

    fn init(href: string = "", tooltip: string? = nil,
            children: table<View> = {}, key: string? = nil)
        self.super(key)
        self.href = href
        self.tooltip = tooltip
        self.children = claimedAll(children)
    end

    fn body(context: BuildContext) -> View
        local router: Router? = RouterScope.of(context)
        local hovered: Binding = context.state("hovered", false)

        local flow: View = FlowStack(children: self.children, spacing: 4.5)

        return GestureArea(
            onHover: (over) => hovered.set(over),
            onTap: () => LinkSpan.follow(router, self.href)
        ) do
            CursorArea(cursor: Cursors.hand) do
                show(flow)
            end
        end
    end

    static local fn follow(router: Router?, href: string) -> nil
        if router == nil then
            -- Reachable, and worth saying so out loud: it means this LinkSpan
            -- was built outside the RouterScope subtree.
            println("LinkSpan: no RouterScope above this link — " .. href)

            return
        end

        router!.navigate(href)
    end
end
```

Three details:

- **Hit area.** A link wrapping several words must hit-test all of them. Wrap
  the run group, not each word — hence `LinkSpan` holding a nested `FlowStack`.
  A link that wraps across a line break will then be two rectangles in one flow,
  which is correct.
- **Underline** is a `Box` under each word, or just a colour in v1.
- **External links** get `style.linkExternal` and `follow` returns early. See
  [`LinkResolver.isExternal`](05-app-shell.md#linkresolver).

Full click path: [Sequence: clicking a link](02-architecture.md#sequence-clicking-a-link).

---

## Block-by-block mapping

| AST | UIKit | Notes |
|---|---|---|
| `Heading(1..6)` | `Text` with `MdTextStyle(size, Face.Bold)` in a `VStack` | H1/H2 get a `Divider` under them. Store the slug on the element for [anchor scrolling](05-app-shell.md#anchors) |
| `Paragraph` | `FlowStack` of word runs | the [inline path](#inlinerenderer) |
| `Code(lang, src)` | `Box(color: codeBg, radius: 6)` → `ScrollView(axis: Horizontal)` → `Text` in `Face.Mono`, `softWrap: false` | horizontal scroll instead of wrapping is what makes code readable; language shown as a small muted label top-right |
| `Quote` | `HStack`: 3px `Box` bar + recursive `VStack` | recurses |
| `List(unordered)` | `VStack` of `HStack(bullet, content)` | bullet is `Text("•")`, or an `Icon` for nesting depth 2+ |
| `List(ordered)` | same, marker `Text(start + i .. ".")` | right-align the numbers in a fixed-width `Frame` so the text lines up |
| `ListItem(checked != nil)` | `Checkbox` (UIKit, `Controls.sau`) + content | read-only: pass no `onChange` |
| `Table` | `Grid` or a `VStack` of `HStack` rows | UIKit's `TableView` is built for typed data rows; a Markdown table's cells are inline content, so a plain `Grid` with your own header styling is the shorter road |
| `Rule` | `Divider` | |
| `Inline.Image` | `Image(...)` via `Graphics.newImage` | resolve `src` relative to the current document's directory, same rule as [`LinkResolver`](05-app-shell.md#linkresolver). Missing file → muted alt text, never a crash |

---

[← The markdown package](03-markdown-package.md) · [Index](README.md) · [The app shell →](05-app-shell.md)
