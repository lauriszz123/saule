# UIKit

A declarative, SwiftUI-flavoured view toolkit for Saule.

```saule
import * from UIKit

-- 1. The entry point of your app
export class CounterApp extends App
    fn body() -> Scene
        return WindowGroup(title: "Counter", width: 420, height: 480) do
            ContentView()
        end
    end
end

-- 2. The user interface
export class ContentView extends View
    fn init(key: string? = nil)
        self.super(key)
    end

    fn body(context: BuildContext) -> View
        -- Keeps track of the app data. When this changes, the UI updates.
        local count: Binding = context.state("count", 0)

        return VStack(spacing: 30.0) do
            Text("Saule Counter").font(34.0).foregroundStyle(Colors.primary)

            Text("" .. count.int())
                .font(80.0)
                .padding(24.0)
                .background(Colors.textMuted.withAlpha(0.1), radius: 80.0)

            HStack(spacing: 40.0) do
                IconButton(icon: Icons.remove, size: 40.0) do
                    count.set(count.int() - 1)
                end

                Button(label: "Reset", color: Colors.surface) do
                    count.set(0)
                end

                IconButton(icon: Icons.add, size: 40.0) do
                    count.set(count.int() + 1)
                end
            end
        end.padding(20.0).centered()
    end
end

class Main
    static fn main()
        CounterApp().run()
    end
end
```

Four things do most of the work there, and all four exist because Saule has
**trailing blocks** — when the last argument to a call is a function, it can be
written after the closing parenthesis as `do … end`:

* children are **statements inside a block**, not a table — no commas, no
  `return`, no closing-parenthesis pile-up. See [ViewBuilder](#viewbuilder);
* everything that wraps a view — padding, a background, alignment — is a
  **modifier chained onto it** rather than a constructor wrapped around it;
* the app is a class you **extend**, with a `body` that returns a `Scene`;
* `context.state` is this kit's `@State`.

Nothing here is a wrapper over something else: `VStack` lays out its children
itself, `View.body` is a real method you override, and the whole thing is
Saule source you can read.

## The model

Two trees, the same split Flutter uses — the names are SwiftUI's, the machinery
underneath is not:

|             |                                                                                                                                                                |
|-------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------|
| **View**    | An immutable description of what the UI should look like. Cheap; thrown away and rebuilt constantly.                                                            |
| **Element** | The persistent instance of a view at one position in the tree. Owns everything mutable: children, laid-out size and offset, any `State`, and a scratch table.   |

When a rebuild produces a new view for a position, the framework reconciles it
against the existing element. Same class *and* same `key` means the element —
and everything it holds — is reused; otherwise the old subtree is unmounted and
a fresh one is mounted.

A view **with a key** is matched against that key anywhere in the old child
list, so reordering a list carries each child's element — and therefore its
state, scratch and scroll position — along with it. Unkeyed views are matched by
position, which keeps a plain list of children cheap. Key the rows of anything
that can be reordered, filtered, or inserted into.

### Where the third tree went

Flutter has a `RenderObject` tree that owns layout and painting. Saule has no
downcasts, so a `PaddingRenderObject` could never read the `Padding` that
configured it — every render class would be stuck holding a `View`-typed field
it cannot inspect.

So the *behaviour* lives on the view instead, as methods that receive the
element holding the mutable state:

```saule
fn layout(el: Element, constraints: Constraints) -> Size
fn paint(el: Element, x: float, y: float) -> nil
fn handleEvent(el: Element, event: PointerEvent, x: float, y: float, inside: boolean) -> boolean
fn handleKey(el: Element, event: KeyEvent) -> boolean
```

Views stay immutable, elements stay generic, and no cast is ever needed. The
defaults implement "transparent wrapper around a single child", so a composite
view only ever overrides `body`.

## ViewBuilder

Children are written as plain statements inside a trailing block:

```saule
VStack(alignment: StackAlignment.Fill, spacing: 14.0) do
    Text("Saule UIKit").font(24.0)
    Counter(label: "Taps", start: 3)
    settingsPanel.expanded()
end
```

SwiftUI gets that from a *result builder*, a compiler feature that rewrites the
block into nested calls. Saule has no such thing, so `ViewBuilder` does the same
job at runtime, in three rules:

1. **`View.init` emits.** Every view ever constructed announces itself to the
   innermost open block. A bare statement like `Text("hi")` therefore records
   itself even though its value is discarded.
2. **Taking a child claims it back.** A view that is now inside another view is
   no longer a sibling, so it comes back out of the block. Every modifier wraps
   in a `SingleChildView`, so `Text("x").padding(4.0)` emits twice and claims
   once — leaving just the `Padding`.
3. **A container opens a block, runs it, and closes it.** What it collected, in
   statement order, is its children.

Rule 2 is the invariant the whole thing rests on: a view that ends up inside
another must be claimed, or it appears twice — once properly and once as a loose
sibling of whatever contains it. That is why every `View`-typed field in this kit
is assigned through `claimed` / `claimedAll`, and why every `Text` modifier goes
through one shared constructor rather than each building its own.

**Claiming searches every open block, innermost first**, not just the top one.
A helper called from inside a block builds its views while *that* block is open,
but the container consuming them usually opens a block of its own first:

```saule
HStack() do                  -- block A opens
    self.chatList()          -- rows are built here, into block A …
end                          -- … but the List that takes them opens block B,
                             --   and claiming only B would leave every row
                             --   sitting in A as well
```

It follows from rule 1 that a helper function works exactly as you would hope:

```saule
local fn chip(label: string) -> View
    return Text(label).padding(3.0).background(Colors.surface, radius: 6.0)
end

VStack() do
    chip("one")      -- only the Surface lands here; the Text and Padding
    chip("two")      -- were claimed on the way out
end
```

And it follows from the same rule that a view constructed inside a block and
then *not* used is still a child. Every builder DSL of this shape behaves that
way. A `throw` out of the middle of a block leaves its frame open, so the build
phase resets the stack before each rebuild.

### Putting an existing view in a block

Only *calls* are collected, because a call is the only moment there is to
notice a view. A view built earlier and held in a local never announced itself
inside the block, and a bare identifier on its own line is not even a statement
— so `show` is the call that says "this one, here":

```saule
local grid: View = TableView(columns: cols, rows: rows)

ScrollView() do
    show(grid)
end
```

Without it that view would sit *outside* the block, and nothing would say so.
`show` claims before it emits, so handing it a view that was built in the same
block moves it to that position rather than listing it twice.

When the slot has a name and there is exactly one child, `child:` says the same
thing more plainly — `ScrollView(child: grid)`.

### children: is still there

A block cannot spell out a list built in a loop, so `children:` remains:

```saule
local chips: table<View> = {}

for name: string in names do
    chips[#chips + 1] = Text(name).padding(3.0)
end

FlowStack(spacing: 6.0, children: chips)
```

Both arrive at the same place — `childrenOf` takes whichever was given, and the
block wins if you somehow pass both.

### Which parameter the block fills

A trailing block binds to **the callee's last function-typed parameter that no
other argument claimed**. That is why `content` is declared last on every
container: a view with its own callback would otherwise swallow the block when
the callback was left at its default. It also means a callback passed by name
frees the block for the children:

```saule
TabView(
    tabs: {"List", "Canvas", "Notes"},
    index: tab,
    onChanged: fn(next: integer)
        select(next)
    end
) do
    ListPane()
    CanvasPane()
    NotesPane()
end
```

Only *calls* are statements, and only views constructed inside the block are
collected. A view built earlier and held in a local goes in as `children:`, or
gets wrapped by something that is a call.

Anything taking a single function takes a block too — controls, dialogs, even
`setState`:

```saule
Button(label: "Save") do showToast(context, "Saved") end
Slider(value: volume) do (next: float) setVolume(next) end
showDialog(context) do return confirmSheet end
```

A single-child slot given several views wraps them in a `VStack`, the way
`ScrollView { … }` does in SwiftUI.

## Modifiers

Every view has a set of chainable methods that return **a new view wrapping
this one**:

```saule
Text("Saved")
    .padding(10.0)
    .background(Colors.success, radius: 6.0)
    .onTapGesture() do
        dismiss()
    end
```

Written as nesting, that same tree reads inside-out: the `Text` you care about
is buried four constructors deep, reading order and paint order disagree, and
the closing parentheses pile up at the bottom. Read a chain bottom-up and you
get the tree, outermost last.

| Modifier | Wraps in |
|---|---|
| `.padding(amount)` / `.padding(insets: …)` | `Padding` |
| `.background(color, radius:, border:, borderWidth:)` | `Surface` |
| `.border(color, width:, radius:)` | `Surface`, fill omitted |
| `.frame(width:, height:)` | `Frame` |
| `.expanded(weight)` | `Expand` |
| `.aligned(alignment)` / `.centered()` | `Align` |
| `.clipped()` | `Clip` |
| `.opacity(value)` | `Opacity` |
| `.onTapGesture(fn)` / `.onSecondaryTapGesture(fn)` / `.onHover(fn)` | `GestureArea` |
| `.help(message)` | `Tooltip` |
| `.cursor(name)` | `CursorArea` |

`Text` has four of its own, and they return `Text` rather than `View` so they
chain with each other:

| Modifier | |
|---|---|
| `.font(size)` | point size |
| `.foregroundStyle(color)` | ink colour |
| `.lineLimit(lines)` | cap the line count, ellipsis on the last |
| `.multilineTextAlignment(align)` | per-line alignment |

The order that does not work is the other way round: `.padding()` returns a
`View`, and a `View` has no `.font`. Style the text first, then wrap it.

They compose in chain order, so `.padding().background()` puts the background
*behind the padded area* and `.background().padding()` puts the padding outside
the background — the same rule SwiftUI has, for the same reason.

Reach for the constructor instead when a view takes several coordinated options
at once. A `GestureArea` wiring up `onTap`, `onTapDown`, `onTapUp` and
`onHover` together is one thing, not four wrappers, and `Button` builds it that
way — see [Button.sau](Button.sau).

### Why there is a registry

`View.padding` wants to build a `Padding`. `Padding` lives in
[Layout.sau](Layout.sau), which imports [Framework.sau](Framework.sau), where
`View` is declared — and Saule rejects circular imports outright rather than
resolving them lazily.

So the direction is inverted. `Framework.sau` declares the *shape* of the
wrappers as a `ViewModifiers` interface and calls through whatever is
installed; [Modifiers.sau](Modifiers.sau) sits above every widget file, can
therefore name all of them, and installs the implementation with a top-level
call that runs when the module is imported. `init.sau` lists it last, so the
implementation is in place long before the first frame.

The cost is one interface hop per modifier. The alternative was moving
`Padding`, `Box`, `Align`, `Clip`, `Expand`, `Opacity`, `GestureArea`,
`CursorArea` and `Tooltip` into `Framework.sau` so `View` could see them,
collapsing six well-separated files into one.

## The app

An app is a class you extend, with a `body` that returns a **`Scene`** rather
than a view:

```saule
export class DemoApp extends App
    fn body() -> Scene
        return WindowGroup(title: "Demo", width: 900, height: 700) do
            Theme(data: ThemeData.dark()) do
                Screen(toolbar: Toolbar(title: "Demo")) do
                    RootView()
                end
            end
        end
    end
end

class Main
    static fn main()
        DemoApp().run()
    end
end
```

`Main.main` stays, because that is the entry Saule actually calls — there are no
attributes to mark one with, and no protocol conformance to hang a program on.
What it does is one line. Everything *about* the app — its window, its size, its
root view — is declared on the app class, instead of being spelled out as
arguments to `runApp` at the call site.

The split from `View` is the one SwiftUI makes: a view's `body` returns views
and runs on every rebuild, an app's `body` returns scenes and runs once. That is
what keeps "the window is 900 wide" from being something a rebuild could change.
`WindowGroup` is the only `Scene` today; the type exists so a second window kind
can arrive without changing the shape of every app that already exists.

`runApp` is still there underneath, and still works on its own if you would
rather open a window imperatively.

## State

Two ways, and the small one comes first.

### context.state

```saule
export class Counter extends View
    fn body(context: BuildContext) -> View
        local count: Binding = context.state("count", 0)

        return Button(label: "tapped " .. count.int()) do
            count.set(count.int() + 1)
        end
    end
end
```

`context.state(name, initial)` is this kit's `@State`: a named slot in the
element's own scratch, so it survives every rebuild at that position, and a
`set` that marks the element dirty — which is the half a plain scratch write
always forgets.

SwiftUI gets the same effect from a property wrapper plus a dependency graph.
Saule has neither, so the value comes back through a `Binding` with typed
readers — `int()`, `number()`, `text()`, `flag()`, and `value()` for anything
else. Each one falls back if the slot somehow holds another type, so a bad write
reverts rather than taking down a frame.

### StatefulView

Reach for this when the state needs lifecycle hooks — `initState`,
`didUpdateView`, `tick`, `dispose` — or when there is enough of it to deserve
typed fields.

## Writing views

### Composite

```saule
export class Badge extends View
    text: string

    fn init(text: string = "", key: string? = nil)
        self.super(key)

        self.text = text
    end

    fn body(context: BuildContext) -> View
        return Text(self.text)
            .padding(insets: EdgeInsets.symmetric(horizontal: 8.0, vertical: 4.0))
            .background(Colors.success, radius: 4.0)
    end
end
```

`View` is the base of everything — there is no separate `StatelessWidget`.
Override `body` and you have a composite; override `layout` / `paint` and you
have a render view.

`State` survives rebuilds. Mutate inside `setState` so the framework knows to
rebuild the subtree.

```saule
export class Counter extends StatefulView
    start: integer

    fn init(start: integer = 0, key: string? = nil)
        self.super(key)

        self.start = start
    end

    fn createState() -> State?
        return CounterState(self.start)
    end
end

class CounterState extends State
    local count: integer

    fn init(start: integer)
        self.super()

        self.count = start
    end

    fn body(context: BuildContext) -> View
        return Button(label: "tapped " .. self.count) do
            self.setState() do
                self.count = self.count + 1
            end
        end
    end
end
```

**Reading configuration from a state.** `State.view` is typed `View` and Saule
cannot downcast, so a state cannot reach its own view's fields through it. Pass
what the state needs into the constructor from `createState()`, as above. The
consequence: a state does not automatically see a *changed* configuration.
Override `didUpdateView` if you need to react to a rebuild.

Lifecycle hooks: `initState`, `didUpdateView(old)`, `tick(dt)`, `dispose`.
`tick` runs once per frame — that is where animations belong.

### unmounted

A plain view gets one hook of its own: `unmounted(el)`, called when its position
in the tree goes away.

**Anything a view opened that outlives its own subtree has to be closed there.**
An overlay layer is the case that matters. Layers are held at the *root*, not
under the view that opened them, so unmounting the opener leaves the layer on
screen — and if the view dismissed it from `tick`, as `Tooltip` does, the tick
that would have cleaned up never comes. The result is a tooltip stuck over a
button that no longer exists, with nothing able to close it.

`Tooltip`, `Picker` and `DateField` all close their layer here. A view of your
own that calls `showMenu` or `showDialog` should keep the handle and do the
same.

### Element scratch

For a couple of interaction booleans, a whole `State` object is overkill.
`context.data()` is a table that belongs to the element, so it survives
rebuilds:

```saule
fn body(context: BuildContext) -> View
    local scratch: table = context.data()
    local hovered: boolean = scratch.hovered == true

    return myContent.onHover() do (inside: boolean)
        scratch.hovered = inside
        context.markNeedsBuild()
    end
end
```

That is exactly how `Button` gets its hover and press states without a `State`
— see [Button.sau](Button.sau).

### Render views

Override `layout` / `paint` when you need to draw or measure directly (and
`handleEvent` / `handleKey` for input). The contract for layout: lay out your
children under constraints you derive, give each one an `offset` relative to
your own top-left, and return your own size.

```saule
export class Underline extends SingleChildView
    fn init(child: View? = nil, key: string? = nil)
        self.super(child, key)
    end

    fn paint(el: Element, x: float, y: float) -> nil
        el.paintChildren(x, y)

        Colors.primary.apply()
        Graphics.line(x, y + el.size.height, x + el.size.width, y + el.size.height)
    end
end
```

`x` / `y` in `paint` are absolute window coordinates; the size is `el.size`.

## Stacks

`VStack`, `HStack` and `ZStack` are the three containers, and `AxisStack` is the
shared base the first two configure — use those rather than it.

```saule
VStack(alignment: StackAlignment.Fill, spacing: 14.0) do … end
HStack(distribution: Distribution.SpaceBetween) do … end
ZStack(alignment: Alignment.bottomRight()) do … end
```

`alignment` positions children on the axis the stack does *not* run along.
`StackAlignment.Leading` is the left edge of a `VStack` and the top edge of an
`HStack` — named for the reading direction rather than a compass point.
`.Fill` stretches every child across that axis, and needs it bounded; on an
unbounded one it behaves like `.Leading`.

`distribution` shares out the slack along the stack's own axis:
`Start`, `End`, `Center`, `SpaceBetween`, `SpaceAround`, `SpaceEvenly`.

`sizing` is `StackSize.Fill` (take everything the parent allows) or
`StackSize.Fit` (shrink-wrap the children).

`.expanded(weight)` on a child makes the stack hand it a share of the leftover
space instead of asking for its natural size; `Spacer()` is the same thing with
nothing in it. `FlowStack` breaks children onto a new line as it narrows, which
an `HStack` cannot do.

## Keyboard and focus

Pointer events carry a `button` — 1 left, 2 right, 3 middle — and all three are
routed. `GestureArea.onSecondaryTapAt` is `fn(x, y)`, which is exactly what
`showMenu` needs:

```saule
GestureArea(
    onSecondaryTapAt: fn(x: float, y: float)
        local origin: Offset = absoluteOf(context.element())
        showMenu(context, origin.dx + x, origin.dy + y, items: {…})
    end,
    child: myList
)
```

Pointer events go wherever the cursor is. Key events go to whatever holds
**focus**, and then bubble up through its ancestors until something claims them
— so a listener wrapped around a screen sees every key the focused control
ignored, without stealing focus from it.

```saule
KeyboardListener(
    onKeyDown: fn(event: KeyEvent)
        if event.ctrl and event.key == "s" then
            save()
            return true
        end
        return false
    end,
    child: myScreen
)
```

`KeyboardListener` keeps working when focus is elsewhere, or nowhere at all —
it registers as a *global* listener, which gets a second pass after the focus
chain declines. Tab skips it, so traversal never parks focus somewhere
invisible.

### Focus

`Focus` is the primitive: it makes a subtree focusable and delivers keys.

```saule
Focus(
    node: myNode,          -- optional handle for programmatic focus
    autofocus: true,
    enabled: true,         -- false drops it from focus and key routing entirely
    wantsText: true,       -- turn on text input while focused
    global: false,         -- also see keys the focus chain ignored
    tabStop: true,         -- whether Tab can land here
    modal: false,          -- trap focus inside this subtree (dialogs)
    onKeyDown: fn(event: KeyEvent) -> boolean
    onKeyUp: fn(event: KeyEvent) -> boolean
    onText: fn(text: string) -> boolean
    onFocusChange: fn(focused: boolean)
    child: …
)
```

It stays a constructor rather than a modifier: those eleven options are one
coordinated unit, and a chain of eleven wrappers would be eleven elements.

Lambda arity is checked in Saule, so those signatures are exact — a callback
with the wrong parameter count is a compile error, not a silent no-op. Return
`true` from a key callback to claim the event.

`FocusNode` moves focus from elsewhere. Create it once and keep it — a node
built fresh each rebuild only works for the rebuild it came from, so store it in
`context.data()` or a `State`:

```saule
local node: FocusNode = …
node.requestFocus()
node.hasFocus()
node.unfocus()
```

`Tab` / `Shift+Tab` walk the focusables in tree order, wrapping at both ends.
This happens only when no view claimed the `Tab` press, so a view that wants
literal tabs simply returns `true`.

Text arrives as its own event kind, already modifier- and layout-corrected
(`shift+a` is `"A"`) — never assemble text out of `Down` events. Key names come
from the engine: `"a"`, `"space"`, `"return"`, `"backspace"`, `"left"`,
`"lshift"`, and so on. `KeyEvent` carries `shift` / `ctrl` / `alt`.

`runApp` enables key repeat by default, which is what editing keys want; pass
`keyRepeat: false` for a game-style app. Text input capture is toggled
automatically — it is on only while a focused view asks for it.

### Text input

`TextField` is a single-line editor: backspace, delete, left, right, home, end,
and return to submit. `Tab` is deliberately left alone so it still moves focus.

```saule
TextField(value: name, placeholder: "Your name") do (text: string)
    name = text                 -- the block is `onChanged`
end

TextField(
    value: name,
    onChanged: fn(text: string)
        name = text
    end
) do (text: string)
    submit()                    -- with `onChanged` named, the block is `onSubmitted`
end
```

`onSubmitted` is *declared before* `onChanged` so that this works out. A block
binds to the last function-typed parameter nothing else claimed, so whichever
callback should be the default has to come last — declared the other way round,
the first form would quietly fill `onSubmitted` and the field would look like it
was ignoring every keystroke. `KeyboardListener` orders `onKeyUp` before
`onKeyDown` for the same reason.

**`value` is nullable, and that is the controlled / uncontrolled switch.**

Pass one and the field is *controlled*: when its owner rebuilds and hands down
text the field is not showing, the field takes it. That is what makes clearing
`draft` after a send, or resetting a form between sign-in and sign-up, actually
clear the box.

Pass nothing and the field is *uncontrolled* and owns its text outright. Its
owner may rebuild as often as it likes — including from `onChanged`, which some
panels do — without disturbing what you are typing.

Defaulting `value` to `""` would collapse those two into one, and an
uncontrolled field whose owner rebuilds would be handed an empty string and
wipe itself on every keystroke. Which is a real bug this kit shipped.

Re-seeding also keys on the *identity* of the view, not just its value. A
keystroke rebuilds the field itself with a stale `value`; only a new view object
from the owner is authoritative. A stale `""` and a deliberate `""` are the same
string, so value alone cannot tell them apart.  after that the field owns its text and
reports edits through `onChanged`. It fills the width it is given — inside an
`HStack`, chain `.expanded()` or `.frame(width: …)`, because a row's main axis
is unbounded.

Shortcuts use **Command on macOS and Control everywhere else**. The engine's key
events carry shift, ctrl and alt but not Command, so `KeyEvent` polls it and
exposes `event.command()` — every editing shortcut asks that rather than `ctrl`,
and both modifiers are accepted. Asking `ctrl` directly is why copy and paste
did not exist on a Mac at all.

Selection works the way it should: shift with any movement key extends it,
Cmd/Ctrl+A selects all, Cmd/Ctrl+C / X / V use the **system clipboard**, and
typing or deleting replaces the selected run. Click to place the caret,
double-click to select a word, drag to select. A plain left or right arrow
collapses a selection to its edge rather than moving a character.

Cmd/Ctrl+arrow moves by word and Cmd/Ctrl+backspace / delete remove one.
**Cmd/Ctrl+Z undoes, Cmd/Ctrl+Y or +Shift+Z redoes** — and a run of typing collapses into a
single undo step, broken at spaces, so undo goes back a word at a time rather
than a letter. History is whole snapshots rather than diffs: the text in a field
is small, and a snapshot can't drift out of sync with the document the way a
replayed diff can.

The editing operations live on `TextEngine`, which you can drive directly if you
are building your own input.

`TextEditor` is the multi-line editor: Enter inserts a newline, up and down move
between lines keeping the column, Home and End work on the current line, and
selection spans lines. Give it a bounded height (`minLines`, an `.expanded()`,
or a `.frame(height: …)`).

Its line mapping comes from `TextStyle.wrapIndices`, which returns line *start
indices* rather than strings — an editor has to map a caret to a line and back,
and re-joined strings lose that correspondence the moment the text contains a
double space. `LineMap` wraps it with `lineAt`, `columnAt`, `indexOf`,
`indexAtPoint` and `pointAt`.

Read-only multi-line text is `Text`, not `TextEditor`: wrapping is on by default
and kicks in whenever the incoming constraint has a bounded width. `maxLines`
caps it with an ellipsis, `align` positions each line, and explicit `\n` always
breaks. In an `HStack` the main axis is unbounded, so text there stays on one
line unless you give it a width. Wrapping costs one measurement per word, so the
result is cached on the element and recomputed only when the text or width
changes.

`Button` is keyboard-operable for free: Tab focuses it, `Enter` or `Space`
presses it, and it draws a focus ring while focused. A disabled button is not
focusable, so it drops out of the Tab order.

Both keep their editing and interaction state in element scratch and blink the
caret straight off the clock in `paint`, so an idle field costs no rebuilds.

## Theme

An ambient bundle of colours, text styles and metrics, read by every control:

```saule
runApp(Theme(data: ThemeData.light(), child: MyApp()))

-- anywhere below it
local theme: ThemeData = Theme.of(context)
Text("hi", style: theme.title)
```

`ThemeData()` gives the dark defaults, `ThemeData.light()` the light ones, and
`copyWith` overrides a few fields without restating the palette. Themes nest —
an inner one shadows the outer for its own subtree.

SwiftUI does this with `@Environment`, Flutter with a generic
`InheritedWidget`; Saule can express neither, because `Theme.of` would have to
return a downcast value. A *concrete* one is fine though. `Theme` writes its
data into element scratch on the way down, and `of` walks up the element chain
reading it. `BuildContext.element()` is there so you can copy the pattern for
any other ambient value your app needs — it is about twenty lines.

## Icons

```saule
Icon(Icons.search)
Icon(Icons.delete, size: 32.0, color: Colors.danger)
IconButton(icon: Icons.settings, tooltip: "Settings") do openSettings() end
```

Glyphs from Google's **Material Icons** font, vendored under `assets/` with its
Apache 2.0 licence. A font was the right shape: one file, any size without
blurring, and recolouring is just `setColor` — PNG sprites would need a sheet
per size, and vector paths would mean hand-writing every icon. (Apple's SF
Symbols are not an option: their licence forbids use off Apple platforms.)

`Icons` names ~70 common glyphs. Any of the font's ~2200 works even if it isn't
named — pass the codepoint, looking it up in
`assets/MaterialIcons-Regular.codepoints`:

```saule
Icon(0xE5CD)
```

`IconFont.setFontPath(path)` moves the font if your assets live elsewhere; a
missing file logs once and draws nothing rather than taking the frame down. The
path is read relative to the project root, not the working directory — see
[Asset paths](#asset-paths).

## Data: grids and tables

```saule
Grid(columns: 3, spacing: 8.0, children: cards)

TableView(
    columns: {
        TableColumn("Name", sortable: true),
        TableColumn("Size", width: 90.0, align: Alignment.centerRight())
    },
    rows: rows,
    sortColumn: column,
    sortOrder: order,
    onSort: fn(column: integer, order: SortOrder) … end
)
```

Both are composed from `HStack`, `VStack`, `Frame` and `.expanded()` rather than
written as render views — columns line up across rows *because* every row is
built from the same width rules, so alignment is structural rather than
something the layout has to keep in sync.

`TableView` rather than `Table`: `Table` is a stdlib module, and shadowing it
inside `UIKit` would break `Table.remove` for anyone importing both.

A `TableColumn` with a `width` is pinned; without one it shares the remainder by
`flex`. Sorting is **reported, not performed**: the table draws the order you
give it and tells you which header was clicked, so the data stays yours. Give
`TableRow` a `key` and reordering carries each row's element with it.

## Controls

```saule
Checkbox(value: on, label: "Notify me") do (next: boolean) … end
Radio(value: "a", groupValue: choice, label: "First") do (next: string) … end
Toggle(value: on) do (next: boolean) … end
Slider(value: volume, min: 0.0, max: 1.0, step: 0.0) do (next: float) … end
ProgressView(value: 0.6)          -- nil for an indeterminate one
Picker(value: region, options: {"A", "B"}) do (next: string) … end
```

```saule
TabView(tabs: {"Details", "History"}, index: current, onChanged: onTab) do
    DetailsPane()
    HistoryPane()
end

DateField(value: due) do (picked: Date) … end
```

`TabView` builds only the selected child, so an expensive pane costs nothing
until you switch to it. The strip is a single focus stop with the arrows moving
between tabs (Home and End jump to the ends), which is how a tab strip is meant
to behave — each tab is deliberately *not* separately focusable.

`DateField` opens a calendar in the overlay. Arrows move a day at a time, Page
Up/Down a month, Enter picks, Escape closes. `Date` is a plain value with
`today()`, `addDays`, `addMonths`, `weekday()`, `daysInMonth()`, `toString()`
(ISO) and `describe()`. The arithmetic goes through days-since-epoch rather than
month-by-month fiddling, so "31 January plus one month" clamps to the 28th and
leap years need no special case.

All of them are controlled: they never keep the value, they hand you the new one
and expect it back. All are focusable, themed, and keyboard-operable — Space or
Enter activates, and `Slider` also takes the arrows, Home and End. `Picker`
opens its list as an overlay entry, so it escapes any scroll view or panel it
sits in.

## Overlays: dialogs, menus, tooltips

A view can only draw inside the box its parent gave it. Anything that has to
escape that — a dialog, a picker's list, a tooltip — goes in the **overlay**: a
list of layers held at the root of the tree, painted above everything and
hit-tested before it.

```saule
local handle: OverlayHandle? = showDialog(context) do
    return VStack(spacing: 12.0) do
        Text("Delete this?")
        Button(label: "Delete", color: Colors.danger) do delete() end
    end
end

handle?.close()
```

`showDialog` hands its content back as a return value, so that outer block does
`return`. The `VStack` inside it is the statement-shaped kind — the two nest
without either changing shape.

`showDialog` centres its content over a dimmed barrier. `showMenu(context, x, y,
items: {…})` anchors a menu at a point and nudges it back on screen near the
edges. `.help("…")` shows a label after a hover delay. All three hand back an
`OverlayHandle` — `close()`, `isOpen()`, `refresh()` — so whoever opened a layer
can close it from anywhere.

**Focus is trapped.** A dialog wraps itself in `Focus(modal: true)`, and while
that is mounted, Tab, autofocus and global shortcuts are all confined to it —
they cannot reach the screen behind. Keys with nothing focused go to the
innermost modal, which is why a menu with no focusable content still sees its
own Escape.

**Clicks are swallowed** by `ModalBarrier`, which also eats the wheel: scrolling
the page under a modal is wrong. Pass `onDismiss` to make clicking outside close
it (menus do, confirm dialogs don't).

For whole screens rather than popups, `Navigator` stacks full-window routes on
the same machinery. A route is **opaque**: it swallows presses and the wheel its
own content did not claim, and confines focus to itself. Covering the window in
paint is not the same as covering it for the pointer — without that, a click on
a blank part of a route lands on whatever the screen below happens to have
there. Toasts stay transparent (that is the point of a toast), and dialogs and
menus bring their own `ModalBarrier`, which also decides what clicking outside
means.


```saule
owner.navigator().push(fn() return SettingsScreen() end)
owner.navigator().pop()          -- also replace(), popToRoot(), canPop()
```

Reach either from a view with `context.owner()?.overlay()` / `?.navigator()`.
Entries are keyed, so opening and closing layers in any order never disturbs the
state inside the others.

## Page structure, and the small pieces

```saule
Screen(toolbar: Toolbar(title: "Inbox", actions: {refreshButton}), footer: statusStrip) do
    MessageList()
end
```

The content takes whatever height is left, so it is the thing that scrolls. The
block fills `content`; the bar and the strip stay named, because a block can
only mean one of the three.

`Divider` and `VerticalDivider` are themed hairlines with `indent` / `endIndent`
for lining up with text rather than the container edge.

`showToast(context, "Saved")` drops a message near the bottom and takes itself
away — no barrier, nothing focusable, so whatever you were doing carries on
underneath. It counts down on the frame clock rather than holding a `State`.

### Cursors

```saule
myField.cursor("ibeam")
```

Names are the engine's: `arrow`, `ibeam`, `crosshair`, `hand`, `grab`,
`resizeleftright`, `resizeupdown`, `resizeall`. **There is no pointing-finger
cursor** — the window backend's macOS set has only an open hand and a closed
one, so `"hand"` is the *drag* hand. Right for something you can pick up, wrong
for a button, which is why nothing clickable here asks for one: macOS shows the
plain arrow over a button.


Nesting resolves the way you'd expect: the innermost region under the pointer
wins. Views *request* a cursor rather than setting one, because several are hit
at once — move events run depth-first with the front-most sibling first, so the
first request in a frame is the innermost, and the frame loop applies it after
routing. The engine call only fires on a change.

Both editors ask for `"ibeam"`; buttons ask for nothing. This is mostly here
for your own views.

## Scrolling

```saule
ScrollView() do
    RowOne()
    RowTwo()
end

List(children: rows, spacing: 2.0)    -- the same thing, plus the VStack
```

The wheel scrolls whichever view is under the pointer. A view only *claims* the
wheel when the offset actually moved, so a list nested inside another one hands
scrolling back to its parent at either end — the way every real UI behaves.

A scroll view needs a bounded extent on its scroll axis: chain `.expanded()` or
`.frame(height: …)`, or put it in anything else that hands down a real height.
Unbounded means "shrink-wrap", and there is then nothing to scroll.

The scrollbar is draggable: grab the thumb, or click the track to page. It only
claims presses inside the strip along the edge, so the rest of the view still
gets its clicks, and it draws nothing at all when the content fits.

`ScrollController` is the position, and the only mutable part:

```saule
local list: ScrollController = ScrollController()
…
List(children: rows, controller: list)
list.jumpTo(0.0)        -- back to the top
list.progress()         -- 0..1, for a custom indicator
list.atEnd()            -- e.g. to trigger "load more"
```

Pass one in when you need to read or drive the position; otherwise the view
keeps its own in element scratch, where it survives rebuilds. Every child is
built up front, so this is for tens of rows, not thousands.

### Fading a subtree

`.opacity(value)` renders the subtree to an offscreen canvas and composites it
once, so a group fades as one image. Per-colour alpha would only fade views that
happen to take a colour, and would double-darken wherever they overlap.

It costs one full-size buffer per faded subtree, and the engine has no way to
free a canvas — so fade groups rather than leaves, and don't put one around
something that resizes every frame.

## Drawing your own pixels

`Canvas` is the escape hatch out of the view system:

```saule
Canvas(height: 200.0, clip: true) do (x: float, y: float, width: float, height: float)
    Colors.primary.apply()
    Graphics.circle("fill", x + width / 2.0, y + height / 2.0, 40.0)
end
```

The painter is `fn(x, y, width, height)`, called every frame with this view's
absolute position and size, and it draws with the raw `Graphics` API. It runs
inside the tree's paint pass, so it inherits the surrounding clip for free, and
its transform is pushed and popped around it.

### The games pattern

For a simulation, pair `paint` with the per-frame **`tick`** hook and skip the
build system entirely:

```saule
export class BouncingBall extends View
    fn tick(el: Element, dt: float) -> nil
        -- advance the simulation; state lives in el.data
    end

    fn paint(el: Element, x: float, y: float) -> nil
        -- draw it
    end
end
```

`tick(el, dt)` runs once per frame on every mounted view, before layout.
Painting happens every frame regardless of rebuilds, so a view that keeps its
state in `el.data` and draws from it costs **zero rebuilds** — the right shape
for a game world with UI layered on top. Call `el.markNeedsBuild()` from `tick`
only when the *view tree* needs to change, not merely the pixels.
`BouncingBall` in [Test.sau](../Test.sau) is a worked example.

## Images and sprites

```saule
Image(path: "assets/logo.png", height: 64.0, fit: ContentMode.Contain)
Image(path: "assets/sheet.png", frame: Rect.cell(3, 16.0, 16.0, 4))
```

PNG only. `ContentMode` is `None`, `Fill`, `Contain` or `Cover`; with no `width`
/ `height` the image takes its natural size, and giving one axis scales the other
to match. `frame` picks a cell out of a spritesheet in image pixels, and
`Rect.cell(index, w, h, columns)` does the grid arithmetic.

Decoding is cached by path in `Images`, so writing `Image(path: …)` inside a
`body` is safe. A missing file logs once and draws nothing rather than taking the
frame down; a file that exists but isn't a valid PNG is a hard error, because an
error inside a native call cannot be caught in Saule.

### Asset paths

```saule
Assets.resolve("assets/logo.png")  --> "/Users/me/UI Project/assets/logo.png"
```

The engine's loaders resolve a path against the **working directory**, which is
wherever the shell happened to be — `saule run` does not move it. So a bare
`"assets/logo.png"` only finds the file when the app was launched from inside
its own folder, and the same project silently loses its icons when run as
`saule run path/to/project`.

`Icon` and `Image` therefore put every relative path through `Assets.resolve`,
which joins it onto `Project.root` — the absolute root the interpreter already
knows, having read `saule.config` to find the entry point. Absolute paths pass
through untouched, and so does anything that isn't under the root: a single-file
script has no project, and there the working-directory reading is all there is.
Use it for your own assets too.

## Animation

`AnimationController` turns the frame clock into an eased 0..1 value over the
existing `Tween` / `EasingTypes`, and `AnimatedBuilder` drives it:

```saule
local fade: AnimationController = AnimationController(
    duration: 0.4,
    easing: EasingTypes.OutCubic
)
fade.forward()          -- also reverse(), toggle(), stop(), reset()

AnimatedBuilder(controller: fade) do (value: float)
    return Frame(width: 200.0 * value, height: 6.0).background(Colors.primary)
end
```

The controller is the state, so keep it somewhere that outlives a rebuild
(`initState`, or `context.data()`). `AnimatedBuilder` has no state of its own and
only marks itself dirty on frames where the value actually moved — a finished
animation costs one comparison per frame, not a rebuild. `lerp`, `lerpColor`,
`lerpOffset`, `lerpSize` and `lerpInsets` read the value out. Set `repeats` or
`autoReverse` for a loop.

## View reference

**Layout** — `Frame`, `Padding`, `Align`, `Center`, `Clip`, `HStack`, `VStack`,
`AxisStack`, `Expand`, `Spacer`, `ZStack`, `Positioned`, `FlowStack`,
`FrameLimits`, `AspectRatio`, `RelativeFrame`

**Shell** — `Screen`, `Toolbar`, `Divider`, `VerticalDivider`, `showToast`

**Data** — `Grid`, `TableView`, `TableColumn`, `TableRow`, `SortOrder`

**Icons** — `Icon`, `IconButton`, `Icons`, `IconFont`

**Overlays** — `showDialog`, `showMenu`, `Tooltip`, `ModalBarrier`, `MenuItem`,
`OverlayHandle`, `Navigator`, `OverlayEntry`, `OverlayManager`

**Scrolling** — `ScrollView`, `List`, `Viewport`, `ScrollBar`, `ScrollController`

**Painting** — `Box`, `Surface`, `Text`, `Canvas`, `Image`, `Opacity`, `Assets`

**Controls** — `Button`, `Checkbox`, `Radio`, `Toggle`, `Slider`,
`ProgressView`, `Picker`, `TabView`, `TabBar`, `DateField`

**Input** — `GestureArea`, `CursorArea`, `Focus`, `KeyboardListener`,
`TextField`, `TextEditor`, `EditableText`, `EditableParagraph`, `TextEngine`,
`LineMap`

**Theme** — `Theme`, `ThemeData`

**App** — `App`, `Scene`, `WindowGroup`, `runApp`

**State** — `Binding`, `State`, `StatefulView`, `ElementData`

**Builder** — `ViewBuilder`, `show`, `childrenOf`, `childOf`, `claimed`,
`claimedAll`

**Modifiers** — `ViewModifiers`, `Modifiers`, `UIKitModifiers`

**Values** — `Size`, `Offset`, `Rect`, `EdgeInsets`, `Alignment`, `Constraints`,
`Color`, `Colors`, `TextStyle`, `PointerEvent`, `KeyEvent`, `FocusNode`,
`ContentMode`, `ScrollAxis`, `TextAlign`, `Date`, `LayoutStats`

**Animation** — `AnimationController`, `AnimatedBuilder`, `Tween`, `EasingTypes`

`Box` owns no layout logic: it composes `Padding`, `Align`, `Surface`, `Frame`
and `Clip`, and every argument you leave out disappears from the tree. It is the
one place a chain of modifiers is worth collapsing into a single constructor,
because a box configured six ways is one idea rather than six wrappers.

### Names, if you came from Flutter

| Flutter | here |
|---|---|
| `Widget` / `StatelessWidget` | `View` |
| `StatefulWidget` | `StatefulView` |
| `build` | `body` |
| `Column` / `Row` / `Stack` | `VStack` / `HStack` / `ZStack` |
| `Flex` | `AxisStack` |
| `Wrap` | `FlowStack` |
| `SizedBox` | `Frame` |
| `Expanded` | `Expand`, or `.expanded()` |
| `ClipRect` | `Clip`, or `.clipped()` |
| `Container` | `Box` |
| `DecoratedBox` | `Surface`, or `.background()` |
| `BoxConstraints` | `Constraints` |
| `ConstrainedBox` | `FrameLimits` |
| `FractionallySizedBox` | `RelativeFrame` |
| `MainAxisAlignment` | `Distribution` |
| `CrossAxisAlignment` | `StackAlignment` (`Leading`/`Center`/`Trailing`/`Fill`) |
| `MainAxisSize` | `StackSize` (`Fit`/`Fill`) |
| `GestureDetector` | `GestureArea`, or `.onTapGesture()` |
| `MouseRegion` | `CursorArea`, or `.cursor()` |
| `CustomPaint` | `Canvas` |
| `SingleChildScrollView` | `ScrollView` |
| `ListView` / `GridView` / `DataTable` | `List` / `Grid` / `TableView` |
| `Tabs` | `TabView` |
| `Dropdown` | `Picker` |
| `Switch` | `Toggle` |
| `ProgressBar` | `ProgressView` |
| `TextArea` | `TextEditor` (and the old `TextEditor` is `TextEngine`) |
| `Scaffold` / `AppBar` | `Screen` / `Toolbar` |
| `BoxFit` | `ContentMode` |

## Density and resizing

The window is resizable, and the tree is laid out in **logical pixels**: on a
150% display a 200-wide button stays visually 200 wide instead of shrinking to
two-thirds. `runApp` reads `Window.getScale()` every frame, lays out at
`physical / scale`, and scales the root transform to match. Pointer coordinates
are converted back down, so hit testing stays in the same coordinate system as
layout.

Text is the part that cannot simply be magnified — a glyph rasterized at 13px
and scaled up is blurry. `TextStyle` builds its font at the *physical* size,
reports measurements divided back to logical units, and draws through
`TextStyle.draw`, which maps the anchor into device space and draws with the
transform reset. `Icon` does the same with its font. Everything else scales
through the root transform, which is what you want for boxes and lines.

`Display.scale()`, `toLogical()` and `toPhysical()` are there if you need the
factor yourself. A `TextStyle` with no explicit `size` uses the engine's default
face, which has no size to rebuild — give type an explicit size if it must stay
sharp on a scaled display.

A resize invalidates every cached layout, so the frame after it re-runs the whole
tree and the next one settles back to zero.

## Notes and limits

- Constants are static **methods**, not fields (`Alignment.center()`,
  `EdgeInsets.all(8.0)`, `Size.zero()`): a static field initializer runs while
  its own class is still being declared, so it cannot reference that class.
  `Colors.primary` is a plain field because `Colors` is a separate class.
- Callbacks carry their full signature — `onChanged: (fn(boolean) -> nil)?`,
  not a bare `function`, which the language no longer has. The parentheses
  matter: without them the `?` attaches to the return type, so
  `fn(boolean) -> nil?` is a callback that is always present and returns a
  nullable nothing, which is not what any of these slots mean.
- A modifier and a field can share a name. `Box` has a `padding` field *and*
  the inherited `.padding()` method; Saule keeps fields and methods in separate
  namespaces, so `self.padding` reads the field and `box.padding(4.0)` calls the
  method. A *method* of the same name is a compile error, which is the case
  worth catching.
- Layout has **relayout boundaries**. An element reuses last frame's size when
  its constraints are unchanged *and* nothing in its subtree was marked dirty, so
  a settled screen does no layout work at all, and one changed view re-lays out
  only its own subtree plus the chain to the root — its siblings hit the cache.
  `LayoutStats.describe()` reports the split if you want to check. The contract
  this rests on: anything that can change layout must mark some element dirty.
  Rebuilds do that automatically; a render view that mutates layout-affecting
  state in `el.data` (a scroll offset, a drag) has to call
  `el.markNeedsLayout()` itself.
- Modifiers add elements. `.padding().background().centered()` is three of them,
  the same three the nested form would have built — but it is easy to chain six
  without noticing. `Box` exists for exactly that case.
- The builder is dynamic. A view constructed inside a block is a child of that
  block whether or not you meant it to be, and only *calls* are collected — a
  bare identifier naming a view built earlier is not one, and would silently
  leave that view outside the block. `show(view)` is the call for that case.
  Both follow from collecting at runtime rather than rewriting the block at
  compile time.
- The focus order is re-derived every frame.
- A `TextStyle` with no `size` binds the engine's default face rather than
  leaving the current font alone — fonts are global state, so inheriting would
  make a view's measured size depend on tree-walk order.
- `StackAlignment.Fill` needs a bounded cross axis; on an unbounded one it
  behaves like `Leading`.
- The old imperative kit (`View` / `Component` / the retained-mode `Widget`) is
  preserved unused under [legacy/](legacy).

## Not here yet

- **Audio.** Nothing in the engine, and it needs a decision first: `cpal` is
  portable but pulls ALSA headers into Linux builds; raw `winmm` costs no
  dependency but is Windows-only.
- **A virtualised list.** `List` builds every child up front, so it is for tens
  of rows, not thousands.
- **Drag and drop**, and `Transform` — the latter needs the pointer
  inverse-transformed on the way down, or a rotated view receives clicks in the
  wrong place.
- **Selectable static text**, and rich text with mixed styles in one paragraph.
- **Text shaping** — no bidi, no ligatures, no combining marks. Measurement is
  per-character against the font atlas.
