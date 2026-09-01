[← The renderer](04-renderer.md) · [Index](README.md) · [Build order →](06-build-order.md)

# 5. The app shell

`md-viewer/src/app/` plus `src/main.sau`. The window, the file tree, and the
navigation that makes [the docs you are reading](README.md) clickable.

- [main.sau](#mainsau)
- [Shell](#shell)
- [Rebuilds](#rebuilds)
- [Router](#router)
- [RouterScope](#routerscope)
- [LinkResolver](#linkresolver)
- [Anchors](#anchors)
- [DocumentStore](#documentstore)
- [Workspace](#workspace)
- [Sidebar](#sidebar)
- [ContentPane](#contentpane)
- [TopBar](#topbar)
- [Keyboard](#keyboard)

---

## main.sau

Registers the fonts, then hands off. Nothing else belongs here — everything
about the app is declared on the `App` subclass, the way
`examples/UI Project/src/main.sau` does it.

```saule
import * from UIKit
import * from Shell
import * from FontRegistry

export class MdViewerApp extends App
    fn body() -> Scene
        return WindowGroup(title: "Markdown", width: 1180, height: 820) do
            Theme(data: ThemeData.light()) do
                Shell(root: MdViewerApp.startingRoot())
            end
        end
    end

    -- `saule run examples/md-viewer -- ~/notes`, or the app's own docs.
    static local fn startingRoot() -> string
        local args: table<string> = Os.args()

        if #args > 0 then
            return args[1]
        end

        return Assets.resolve("docs")
    end
end

class Main
    static fn main()
        FontRegistry.register(Face.Regular, "assets/Inter-Regular.ttf")
        FontRegistry.register(Face.Bold, "assets/Inter-Bold.ttf")
        FontRegistry.register(Face.Italic, "assets/Inter-Italic.ttf")
        FontRegistry.register(Face.BoldItalic, "assets/Inter-BoldItalic.ttf")
        FontRegistry.register(Face.Mono, "assets/JetBrainsMono-Regular.ttf")
        IconFont.setFontPath("assets/MaterialIcons-Regular.ttf")

        MdViewerApp().run()
    end
end
```

`Assets.resolve` makes a path absolute against `Project.root`, so the app works
regardless of the shell's working directory. Without it,
`saule run examples/md-viewer` from the repo root finds no fonts and no docs.
See UIKit's `Assets.sau` for why.

---

## Shell

The three-region layout, and the **owner of every long-lived object**. Router,
store and workspace all go in element state so they survive the rebuild that
navigation triggers — see [Who owns what](02-architecture.md#who-owns-what).

```saule
export class Shell extends View
    root: string

    fn body(context: BuildContext) -> View
        -- One slot each, created on first build and reused forever after.
        local state: ElementData = context.data()

        if state.scratch.store == nil then
            state.scratch.store = DocumentStore(self.root)
            state.scratch.workspace = Workspace(self.root)

            local router: Router = Router(Location(Workspace.entryFile(self.root), nil))
            router.onChange = () => context.markNeedsBuild()
            state.scratch.router = router
        end

        local router: Router = state.scratch.router as Router
        local store: DocumentStore = state.scratch.store as DocumentStore
        local workspace: Workspace = state.scratch.workspace as Workspace

        return RouterScope(router: router) do
            VStack(spacing: 0.0) do
                TopBar(router: router, store: store)
                Divider()
                HStack(spacing: 0.0) do
                    Sidebar(tree: workspace.tree, current: router.current)
                        .frame(width: 260.0)
                    VerticalDivider()
                    ContentPane(router: router, store: store).expanded()
                end.expanded()
            end
        end
    end
end
```

`context.state(...)` would work here too and is more idiomatic for a single
value, but it hands back a `Binding` — a typed-getter wrapper for primitives.
For three class instances that are read and never reassigned, a guarded
`scratch` write is the honest version, recovered with `as` on the way out.

---

## Rebuilds

**Mutating the router repaints nothing.** UIKit rebuilds a subtree when an
element is marked dirty, and a plain field write on an object in scratch is
invisible to that.

So `Router` carries an `onChange` closure, and `Shell` sets it to
`() => context.markNeedsBuild()` on the `Shell`'s own context. Every state
change — `open`, `navigate`, `back`, `forward` — calls it last.

Mark the `Shell`, not the `ContentPane`: the top bar's back/forward buttons and
the sidebar's highlight both depend on the current location too. Rebuilding the
whole shell is a few hundred views and happens on a click, not per frame.

---

## Router

Addresses and history. This is **not** UIKit's `Navigator` — that is an overlay
route stack for modals and dialogs, which is a different problem with a
similar name.

```saule
export class Location
    path: string        -- absolute file path
    anchor: string?     -- heading slug, no "#"

    fn init(path: string, anchor: string? = nil)
        self.path = path
        self.anchor = anchor
    end

    fn equals(other: Location) -> boolean
        return self.path == other.path and self.anchor == other.anchor
    end
end

export class Router
    current: Location
    onChange: (fn() -> nil)?

    local backStack: table<Location>
    local fwdStack: table<Location>

    fn init(start: Location)
        self.current = start
        self.backStack = {}
        self.fwdStack = {}
    end

    -- Go to an absolute location. The sidebar uses this.
    fn open(target: Location) -> nil
        if target.equals(self.current) then
            return
        end

        Table.insert(self.backStack, self.current)
        Table.clear(self.fwdStack)
        self.current = target
        self.changed()
    end

    -- Follow an href from the current document. Links use this.
    fn navigate(href: string) -> nil
        if LinkResolver.isExternal(href) then
            return
        end

        local target: Location? = LinkResolver.resolve(self.current, href)

        if target == nil then
            println("Router: unresolved link " .. href)

            return
        end

        self.open(target!)
    end

    fn back() -> nil
        local previous: Location? = Table.remove(self.backStack)

        if previous == nil then
            return
        end

        Table.insert(self.fwdStack, self.current)
        self.current = previous!
        self.changed()
    end

    fn forward() -> nil
        local next: Location? = Table.remove(self.fwdStack)

        if next == nil then
            return
        end

        Table.insert(self.backStack, self.current)
        self.current = next!
        self.changed()
    end

    fn canBack() -> boolean
        return #self.backStack > 0
    end

    fn canForward() -> boolean
        return #self.fwdStack > 0
    end

    local fn changed() -> nil
        local notify: (fn() -> nil)? = self.onChange

        if notify != nil then
            notify!()
        end
    end
end
```

The rules that matter, drawn out in
[State: history](02-architecture.md#state-history):

- **`open` clears the forward stack.** Going back and then somewhere new must
  not leave the abandoned branch reachable.
- **An anchor-only jump is still history.** `Location.equals` compares the
  anchor too, so `#the-ast` from the top of the same page pushes an entry and
  back returns you to where you were reading.
- **Cap the back stack** at, say, 200 entries. A reader clicking around for an
  hour should not grow it without bound.

---

## RouterScope

The ambient lookup, copied from `Theme` — whose own source says to copy it:

> Copy the pattern for any other ambient value your app needs; it is about
> twenty lines.

`Theme` gets a typed `data.theme` field on `ElementData`; an app-side value has
to use the untyped `scratch` bag and recover with `as`. Same twenty lines
otherwise.

```saule
export class RouterScope extends View
    router: Router
    child: View?

    fn init(router: Router, child: View? = nil,
            key: string? = nil, content: (fn() -> nil)? = nil)
        self.super(key)
        self.router = router
        self.child = childOf(child, content)
    end

    -- The nearest router above `context`, or nil when there is none.
    static fn of(context: BuildContext) -> Router?
        local node: Element? = context.element()

        while node != nil do
            local found = node!.data.scratch.mdRouter

            if found != nil then
                return found as Router
            end

            node = node!.parent
        end

        return nil
    end

    fn body(context: BuildContext) -> View
        -- Written before the child builds, which is the whole trick: this
        -- element's scratch is populated by the time anything below calls `of`.
        context.data().scratch.mdRouter = self.router

        return self.child ?? Frame()
    end
end
```

**The failure mode:** a `LinkSpan` built outside the `RouterScope` subtree gets
`nil` and its clicks do nothing, silently. That is why
[`LinkSpan.follow`](04-renderer.md#links) prints when the router is missing —
a click that does nothing is otherwise indistinguishable from a broken hit
test, and you will spend an hour on the wrong one.

---

## LinkResolver

`href` plus the current location, out comes a `Location`. Pure functions, no
state, trivially testable — [test it headlessly](07-testing.md).

```saule
export class LinkResolver
    static fn isExternal(href: string) -> boolean
        return String.contains(href, "://")
            or String.starts(href, "mailto:")
            or String.starts(href, "//")
    end

    static fn resolve(from: Location, href: string) -> Location?
end
```

`resolve` in order:

1. **Split the anchor.** Everything after the first `#`. An href of `"#the-ast"`
   alone means *this document*: return `Location(from.path, "the-ast")`.
2. **Empty path after the split** → same document, new anchor.
3. **Absolute path** (leading `/`, or a Windows drive letter) → use as-is.
4. **Relative path** → join against the *directory* of `from.path`, not
   `from.path` itself. `docs/02-architecture.md` + `04-renderer.md` =
   `docs/04-renderer.md`.
5. **Normalize.** Split on `/`, drop `.` segments, pop one segment per `..`,
   rejoin. Do this textually — do not shell out to a real path canonicaliser,
   which would resolve symlinks and give you a path that no longer matches the
   sidebar's.
6. **Percent-decode** `%20` and friends, so `My%20Notes.md` finds the file.
7. **Directory link** (`../markdown/`) → try `README.md` inside it, the way
   every Git host does.
8. **Extensionless** (`../../../README`) → try `.md` appended.
9. **Check `Os.exists`.** Missing file → return `nil` and let the router log it.

Steps 7 and 8 matter for a docs folder: cross-references written for GitHub
routinely omit the `.md`.

Windows: split on both `/` and `\` when normalizing, and join with `/`. The
engine and `Io` both accept forward slashes there.

---

## Anchors

Clicking `[the AST](02-architecture.md#the-ast)` must land on that heading.

The parser already put a `slug` on every `Block.Heading` and a `HeadingRef`
list on the [`Document`](03-markdown-package.md#the-ast), so the pieces exist.
Scrolling to one takes three steps:

1. **Render each heading with a stable key**, `"h:" .. slug`, so its element
   survives rebuilds and can be found.
2. **After layout**, read the heading element's offset relative to the scroll
   content. UIKit's `Controls.sau` exports `absoluteOf(element) -> Offset` for
   exactly this — it is what the `Picker` uses to place its popup.
3. **Jump the controller**: `ScrollController.jumpTo(y)` on the content pane's
   controller, obtained with `controllerFor(element.data)`.

The ordering trap: on the frame where a *new document* is first built, nothing
has been laid out yet, so the offset is zero. Resolve the anchor in `tick` on
the frame **after** the document changes, not during `body`. Keep a
`pendingAnchor` in the `ContentPane`'s scratch, consume it in `tick` once
`el.size.height > 0.0`, and clear it.

Simplest correct v1: if resolving the anchor is not ready, scroll to top and
try again next frame. Two frames of delay is invisible; a wrong scroll position
is not.

---

## DocumentStore

The only component that touches the filesystem, and the parse cache.

```saule
export class DocumentStore
    root: string
    local cache: table          -- path → Document
    local failed: table         -- path → true

    fn get(path: string) -> Document?
        local hit = self.cache[path]

        if hit != nil then
            return hit as Document
        end

        if self.failed[path] != nil then
            return nil
        end

        local source: string? = DocumentStore.readAll(path)

        if source == nil then
            self.failed[path] = true

            return nil
        end

        local parsed: Document = Markdown.parse(source!)
        self.cache[path] = parsed

        return parsed
    end

    static local fn readAll(path: string) -> string?
        local handle: File? = Io.open(path, IoMode.Read)

        if handle == nil then
            return nil
        end

        local text: string? = handle!.read("a")
        handle!.close()

        return text
    end

    fn invalidate(path: string) -> nil
        self.cache[path] = nil
        self.failed[path] = nil
    end
end
```

`get` is called from `ContentPane.body` — **every rebuild**. Without the cache
you would reparse a 90 KB document on every hover state change in the sidebar.
With it, parsing happens once per file per session.

`invalidate` is unused in v1 and is the entire hook a
[live-reload](01-scope.md#what-v1-deliberately-does-not-do) feature needs later:
poll `Os.fsInfo(path).modifiedAt`, invalidate on change, `markNeedsBuild`.

---

## Workspace

The file tree.

```saule
export class FileNode
    name: string
    path: string
    isDir: boolean
    children: table<FileNode>
end

export class Workspace
    root: string
    tree: FileNode
end
```

Scanning, with the details that matter:

- `Os.list(path)` **throws** when a directory cannot be read — it is the one
  filesystem function in the stdlib that does. Wrap the call in
  `try / catch e: string` and treat a failure as an empty directory. An
  unreadable folder somewhere under `~/notes` must not take the app down.
- `Os.fsInfo(child)?.kind` distinguishes files from directories.
- Keep only `.md` and `.markdown` files, plus directories that contain at least
  one (recursively) — otherwise `node_modules` shows up as a hundred empty
  folders.
- Skip dotfiles and dot-directories.
- Sort: directories first, then files, each `String.lower`-cased so `README.md`
  and `api.md` sort sensibly together.
- **Bound the recursion.** A depth cap of ~8 and a total-node cap of a few
  thousand turns "user opened `/`" from a hang into a truncated tree.

`Workspace.entryFile(root)` picks the initial document: `README.md` if present,
else `index.md`, else the first `.md` in sorted order, else a synthetic empty
document with a "no Markdown files here" message. Handle that last case — it is
the first thing that happens when someone points the app at the wrong folder.

---

## Sidebar

A `ScrollView` over a `VStack` of rows, built by walking `FileNode`.

- One row per node: `HStack(Icon, Text)` wrapped in a `GestureArea` calling
  `router.open(Location(node.path, nil))`.
- Indent by depth: `Frame(width: 12.0 * depth)` leading each row.
- Directories toggle open/closed. Keep the expanded set in the sidebar's own
  `context.state` keyed by path — **not** on `FileNode`, so a rescan does not
  collapse everything.
- Highlight the row whose path equals `router.current.path`: a `Box` behind it
  in `Colors.surface`, plus a bolder face.
- Prefer `Document.title` (the first H1) over the filename when the file is
  already in the store's cache. Do not parse a file just to get its title —
  the sidebar would parse the whole tree on startup.
- Row height ~26, font ~13.

For a folder with thousands of files, swap the `VStack` for UIKit's `List`
(`Scroll.sau`), which virtualizes. Not needed for a docs folder.

---

## ContentPane

```saule
fn body(context: BuildContext) -> View
    local location: Location = self.router.current
    local document: Document? = self.store.get(location.path)

    if document == nil then
        return MissingDocument(path: location.path).centered()
    end

    return ScrollView(key: "doc:" .. location.path) do
        DocumentView(document!)
            .padding(horizontal: 48.0, vertical: 32.0)
            .frame(maxWidth: 760.0)
    end
end
```

Two things here are load-bearing:

- **`key: "doc:" .. path`.** The scroll offset lives in a `ScrollController` in
  element scratch, and an unkeyed `ScrollView` at the same tree position is
  reused across documents — so a new file would open at the old file's scroll
  position. Keying by path gives each document its own element and its own
  offset. Keying by path *without* the anchor is deliberate: an anchor jump
  within a document must not reset the scroll and then fight the jump.
- **`maxWidth: 760`.** Text measured against a 1200px-wide window wraps at a
  line length nobody can read. Cap the measure width; centre the column.

`MissingDocument` is a small centred view: the path, an icon, and "not found".
It is reached by a broken link in someone's README, which is common enough to
deserve a real screen rather than a blank pane.

---

## TopBar

`Toolbar` (UIKit `Shell.sau`) holding:

- **Back** — `IconButton(icon: Icons.arrowBack)`, disabled when
  `not router.canBack()`. Render disabled as `Colors.textMuted` and pass no
  callback, rather than passing one that returns early — a button that looks
  live and does nothing reads as a bug.
- **Forward** — `Icons.arrowForward`, same treatment.
- **Breadcrumb** — the current path relative to the workspace root, split on
  `/`, joined with a muted `›`. Each segment can be clickable in v1.1; plain
  text is fine for v1.
- Optionally a light/dark toggle on the right, flipping the `ThemeData` the
  `Theme` in [`main.sau`](#mainsau) is given. That means lifting the theme
  choice into `Shell` state — a five-line change, and it is the fastest way to
  prove `MarkdownStyle.from(theme)` is actually reading the theme.

---

## Keyboard

Wrap the shell in `KeyboardListener` (UIKit `Gesture.sau`) and handle:

| Key | Action |
|---|---|
| `Backspace` / `Alt+Left` | `router.back()` |
| `Alt+Right` | `router.forward()` |
| `Home` / `End` | scroll to top / bottom |
| `PageUp` / `PageDown` | scroll by a viewport |
| `Cmd/Ctrl+F` | reserved for v2 search |

`KeyboardListener` extends `Focus`, so the shell must actually hold focus for
these to arrive — request it on mount. Mouse-wheel scrolling already works via
`ScrollView` and needs nothing.

---

[← The renderer](04-renderer.md) · [Index](README.md) · [Build order →](06-build-order.md)
