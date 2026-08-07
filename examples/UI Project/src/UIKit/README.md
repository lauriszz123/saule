# UIKit

A declarative, Flutter-style widget toolkit for Saule.

```saule
import * from UIKit

export class Hello extends StatelessWidget
    fn build(context: BuildContext) -> Widget
        return Center(
            child: Container(
                padding: EdgeInsets.all(20.0),
                color: Colors.surfaceLight,
                radius: 10.0,
                child: Column(
                    mainAxisSize: MainAxisSize.Min,
                    spacing: 12.0,
                    children: {
                        Text("Hello", style: TextStyle(color: Colors.text, size: 24.0)),
                        Button(label: "Click me", onPressed: fn() println("hi") end)
                    }
                )
            )
        )
    end
end

class Main
    static fn main()
        runApp(Hello(), width: 800, height: 600, title: "Hello")
    end
end
```

## The model

Two trees, the same split Flutter uses:

|             |                                                                                                                                                                 |
|-------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------|
| **Widget**  | An immutable description of what the UI should look like. Cheap; thrown away and rebuilt constantly.                                                            |
| **Element** | The persistent instance of a widget at one position in the tree. Owns everything mutable: children, laid-out size and offset, any `State`, and a scratch table. |

When a rebuild produces a new widget for a position, the framework reconciles it against the existing element. Same
class *and* same `key` means the element — and everything it holds — is reused; otherwise the old subtree is unmounted
and a fresh one is mounted.

A widget **with a key** is matched against that key anywhere in the old child list, so reordering a list carries each
child's element — and therefore its state, scratch and scroll position — along with it. Unkeyed widgets are matched by
position, which keeps a plain list of children cheap. Key the rows of anything that can be reordered, filtered, or
inserted into.

### Where the third tree went

Flutter has a `RenderObject` tree that owns layout and painting. Saule has no downcasts, so a `PaddingRenderObject`
could never read the `Padding` that configured it — every render class would be stuck holding a `Widget`-typed field it
cannot inspect.

So the *behaviour* lives on the widget instead, as methods that receive the element holding the mutable state:

```saule
fn layout(el: Element, constraints: BoxConstraints) -> Size
fn paint(el: Element, x: float, y: float) -> nil
fn handleEvent(el: Element, event: PointerEvent, x: float, y: float, inside: boolean) -> boolean
fn handleKey(el: Element, event: KeyEvent) -> boolean
```

Widgets stay immutable, elements stay generic, and no cast is ever needed. The defaults implement "transparent wrapper
around a single child", so a composite widget only ever overrides `build`.

## Writing widgets

### Stateless

```saule
export class Badge extends StatelessWidget
    text: string

    fn init(text: string = "", key: string? = nil)
        self.super(key)

        self.text = text
    end

    fn build(context: BuildContext) -> Widget
        return Container(
            color: Colors.success,
            radius: 4.0,
            padding: EdgeInsets.symmetric(horizontal: 8.0, vertical: 4.0),
            child: Text(self.text)
        )
    end
end
```

### Stateful

`State` survives rebuilds. Mutate inside `setState` so the framework knows to rebuild the subtree.

```saule
export class Counter extends StatefulWidget
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

    fn build(context: BuildContext) -> Widget
        return Button(
            label: "tapped " .. self.count,
            onPressed: fn()
                self.setState(fn()
                    self.count = self.count + 1
                end)
            end
        )
    end
end
```

**Reading configuration from a state.** `State.widget` is typed `Widget` and Saule cannot downcast, so a state cannot
reach its own widget's fields through it. Pass what the state needs into the constructor from `createState()`, as above.
The consequence: a state does not automatically see a *changed*
configuration. Override `didUpdateWidget` if you need to react to a rebuild.

Lifecycle hooks: `initState`, `didUpdateWidget(old)`, `tick(dt)`, `dispose`.
`tick` runs once per frame — that is where animations belong.

### Element scratch

For a couple of interaction booleans, a whole `State` object is overkill.
`context.data()` is a table that belongs to the element, so it survives rebuilds:

```saule
fn build(context: BuildContext) -> Widget
    local scratch: table = context.data()
    local hovered: boolean = scratch.hovered == true

    return GestureDetector(
        onHover: fn(inside: boolean)
            scratch.hovered = inside
            context.markNeedsBuild()
        end,
        child: ...
    )
end
```

That is exactly how `Button` gets its hover and press states while staying stateless — see [Button.sau](Button.sau).

### Render widgets

Override `layout` / `paint` when you need to draw or measure directly (and
`handleEvent` / `handleKey` for input). The contract for layout: lay out your children under constraints you derive,
give each one an `offset` relative to your own top-left, and return your own size.

```saule
export class Underline extends SingleChildWidget
    fn init(child: Widget? = nil, key: string? = nil)
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

## Keyboard and focus

Pointer events carry a `button` — 1 left, 2 right, 3 middle — and all three are routed.
`GestureDetector.onSecondaryTapAt` is `fn(x, y)`, which is exactly what
`showMenu` needs:

```saule
GestureDetector(
    onSecondaryTapAt: fn(x: float, y: float)
        local origin: Offset = absoluteOf(context.element())
        showMenu(context, origin.dx + x, origin.dy + y, items: {...})
    end,
    child: myList
)
```

Pointer events go wherever the cursor is. Key events go to whatever holds **focus**, and then bubble up through its
ancestors until something claims them — so a listener wrapped around a screen sees every key the focused control
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

`KeyboardListener` keeps working when focus is elsewhere, or nowhere at all — it registers as a *global* listener, which
gets a second pass after the focus chain declines. Tab skips it, so traversal never parks focus somewhere invisible.

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
    child: ...
)
```

Lambda arity is checked in Saule, so those signatures are exact — a callback with the wrong parameter count is a compile
error, not a silent no-op. Return
`true` from a key callback to claim the event.

`FocusNode` moves focus from elsewhere. Create it once and keep it — a node built fresh each rebuild only works for the
rebuild it came from, so store it in `context.data()` or a `State`:

```saule
local node: FocusNode = ...
node.requestFocus()
node.hasFocus()
node.unfocus()
```

`Tab` / `Shift+Tab` walk the focusables in tree order, wrapping at both ends. This happens only when no widget claimed
the `Tab` press, so a widget that wants literal tabs simply returns `true`.

Text arrives as its own event kind, already modifier- and layout-corrected (`shift+a` is `"A"`) — never assemble text
out of `Down` events. Key names come from the engine: `"a"`, `"space"`, `"return"`, `"backspace"`, `"left"`,
`"lshift"`, and so on. `KeyEvent` carries `shift` / `ctrl` / `alt`.

`runApp` enables key repeat by default, which is what editing keys want; pass
`keyRepeat: false` for a game-style app. Text input capture is toggled automatically — it is on only while a focused
widget asks for it.

### Text input

`TextField` is a single-line editor: backspace, delete, left, right, home, end, and return to submit. `Tab` is
deliberately left alone so it still moves focus.

```saule
TextField(
    placeholder: "Your name",
    autofocus: true,
    onChanged: fn(text: string)
        println(text)
    end,
    onSubmitted: fn(text: string)
        submit(text)
    end
)
```

`value` seeds the initial contents; after that the field owns its text and reports edits through `onChanged`. It fills
the width it is given — inside a
`Row`, wrap it in `Expanded` or a `SizedBox`, because a row's main axis is unbounded.

Selection works the way it should: shift with any movement key extends it, Ctrl+A selects all, Ctrl+C / Ctrl+X / Ctrl+V
use the **system clipboard**, and typing or deleting replaces the selected run. Click to place the caret, double-click
to select a word, drag to select. A plain left or right arrow collapses a selection to its edge rather than moving a
character.

Ctrl+arrow moves by word and Ctrl+backspace / Ctrl+delete remove one. **Ctrl+Z undoes, Ctrl+Y or Ctrl+Shift+Z redoes** —
and a run of typing collapses into a single undo step, broken at spaces, so undo goes back a word at a time rather than
a letter. History is whole snapshots rather than diffs: the text in a field is small, and a snapshot can't drift out of
sync with the document the way a replayed diff can.

The editing operations live on `TextEditor`, which you can drive directly if you are building your own input.

`TextArea` is the multi-line editor: Enter inserts a newline, up and down move between lines keeping the column, Home
and End work on the current line, and selection spans lines. Give it a bounded height (`minLines`, an `Expanded`, or a
`SizedBox`).

Its line mapping comes from `TextStyle.wrapIndices`, which returns line *start indices* rather than strings — an editor
has to map a caret to a line and back, and re-joined strings lose that correspondence the moment the text contains a
double space. `LineMap` wraps it with `lineAt`, `columnAt`, `indexOf`,
`indexAtPoint` and `pointAt`.

Read-only multi-line text is `Text`, not `TextArea`: wrapping is on by default and kicks in whenever the incoming
constraint has a bounded width. `maxLines` caps it with an ellipsis, `align` positions each line, and explicit `\n`
always breaks. In a
`Row` the main axis is unbounded, so text there stays on one line unless you give it a width. Wrapping costs one
measurement per word, so the result is cached on the element and recomputed only when the text or width changes.

`Button` is keyboard-operable for free: Tab focuses it, `Enter` or `Space`
presses it, and it draws a focus ring while focused. A disabled button is not focusable, so it drops out of the Tab
order.

Both keep their editing and interaction state in element scratch and blink the caret straight off the clock in `paint`,
so an idle field costs no rebuilds.

## Theme

An ambient bundle of colours, text styles and metrics, read by every control:

```saule
runApp(Theme(data: ThemeData.light(), child: MyApp()))

-- anywhere below it
local theme: ThemeData = Theme.of(context)
Text("hi", style: theme.title)
```

`ThemeData()` gives the dark defaults, `ThemeData.light()` the light ones, and
`copyWith` overrides a few fields without restating the palette. Themes nest — an inner one shadows the outer for its
own subtree.

Flutter does this with a generic `InheritedWidget`, which Saule can't express:
`Theme.of` would have to return a downcast value. A *concrete* one is fine though. `Theme` writes its data into element
scratch on the way down, and `of`
walks up the element chain reading it. `BuildContext.element()` is there so you can copy the pattern for any other
ambient value your app needs — it is about twenty lines.

## Icons

```saule
Icon(Icons.search)
Icon(Icons.delete, size: 32.0, color: Colors.danger)
IconButton(icon: Icons.settings, tooltip: "Settings", onPressed: ...)
```

Glyphs from Google's **Material Icons** font, vendored under `assets/` with its Apache 2.0 licence. A font was the right
shape: one file, any size without blurring, and recolouring is just `setColor` — PNG sprites would need a sheet per
size, and vector paths would mean hand-writing every icon. (Apple's SF Symbols are not an option: their licence forbids
use off Apple platforms.)

`Icons` names ~70 common glyphs. Any of the font's ~2200 works even if it isn't named — pass the codepoint, looking it
up in
`assets/MaterialIcons-Regular.codepoints`:

```saule
Icon(0xE5CD)
```

`IconFont.setFontPath(path)` moves the font if your assets live elsewhere; a missing file logs once and draws nothing
rather than taking the frame down.

## Data: grids and tables

```saule
GridView(columns: 3, spacing: 8.0, children: cards)

DataTable(
    columns: {
        TableColumn("Name", sortable: true),
        TableColumn("Size", width: 90.0, align: Alignment.centerRight())
    },
    rows: rows,
    sortColumn: column,
    sortOrder: order,
    onSort: fn(column: integer, order: SortOrder) ... end
)
```

Both are composed from `Row`, `Column`, `SizedBox` and `Expanded` rather than written as render widgets — columns line
up across rows *because* every row is built from the same width rules, so alignment is structural rather than something
the layout has to keep in sync.

A `TableColumn` with a `width` is pinned; without one it shares the remainder by
`flex`. Sorting is **reported, not performed**: the table draws the order you give it and tells you which header was
clicked, so the data stays yours. Give
`TableRow` a `key` and reordering carries each row's element with it.

## Controls

```saule
Checkbox(value: on, label: "Notify me", onChanged: fn(next: boolean) ... end)
Radio(value: "a", groupValue: choice, label: "First", onChanged: fn(next: string) ... end)
Switch(value: on, onChanged: fn(next: boolean) ... end)
Slider(value: volume, min: 0.0, max: 1.0, step: 0.0, onChanged: fn(next: float) ... end)
ProgressBar(value: 0.6)          -- nil for an indeterminate one
Dropdown(value: region, options: {"A", "B"}, onChanged: fn(next: string) ... end)
```

```saule
Tabs(
    tabs: {"Details", "History"},
    index: current,
    children: {detailsPane, historyPane},
    onChanged: fn(next: integer) ... end
)

DateField(value: due, onChanged: fn(picked: Date) ... end)
```

`Tabs` builds only the selected child, so an expensive pane costs nothing until you switch to it. The strip is a single
focus stop with the arrows moving between tabs (Home and End jump to the ends), which is how a tab strip is meant to
behave — each tab is deliberately *not* separately focusable.

`DateField` opens a calendar in the overlay. Arrows move a day at a time, Page Up/Down a month, Enter picks, Escape
closes. `Date` is a plain value with
`today()`, `addDays`, `addMonths`, `weekday()`, `daysInMonth()`, `toString()`
(ISO) and `describe()`. The arithmetic goes through days-since-epoch rather than month-by-month fiddling, so "31 January
plus one month" clamps to the 28th and leap years need no special case.

All of them are controlled: they never keep the value, they hand you the new one and expect it back. All are focusable,
themed, and keyboard-operable — Space or Enter activates, and `Slider` also takes the arrows, Home and End.
`Dropdown` opens its list as an overlay entry, so it escapes any scroll view or panel it sits in.

## Overlays: dialogs, menus, tooltips

A widget can only draw inside the box its parent gave it. Anything that has to escape that — a dialog, a dropdown, a
tooltip — goes in the **overlay**: a list of layers held at the root of the tree, painted above everything and
hit-tested before it.

```saule
local handle: OverlayHandle? = showDialog(
    context,
    builder: fn()
        return Column(children: {Text("Delete this?"), buttons})
    end
)
handle?.close()
```

`showDialog` centres its content over a dimmed barrier. `showMenu(context, x, y,
items: {...})` anchors a menu at a point and nudges it back on screen near the edges.
`Tooltip(message: "...", child: ...)` shows a label after a hover delay. All three hand back an `OverlayHandle` —
`close()`, `isOpen()`, `refresh()` — so whoever opened a layer can close it from anywhere.

**Focus is trapped.** A dialog wraps itself in `Focus(modal: true)`, and while that is mounted, Tab, autofocus and
global shortcuts are all confined to it — they cannot reach the screen behind. Keys with nothing focused go to the
innermost modal, which is why a menu with no focusable content still sees its own Escape.

**Clicks are swallowed** by `ModalBarrier`, which also eats the wheel: scrolling the page under a modal is wrong. Pass
`onDismiss` to make clicking outside close it (menus do, confirm dialogs don't).

For whole screens rather than popups, `Navigator` stacks full-window routes on the same machinery:

```saule
owner.navigator().push(fn() return SettingsScreen() end)
owner.navigator().pop()          -- also replace(), popToRoot(), canPop()
```

Reach either from a widget with `context.owner()?.overlay()` /
`?.navigator()`. Entries are keyed, so opening and closing layers in any order never disturbs the state inside the
others.

## Page structure, and the small pieces

```saule
Scaffold(
    appBar: AppBar(title: "Inbox", actions: {refreshButton}),
    body: messageList,
    footer: statusStrip
)
```

The body takes whatever height is left, so it is the thing that scrolls.
`Divider` and `VerticalDivider` are themed hairlines with `indent` / `endIndent`
for lining up with text rather than the container edge.

`showToast(context, "Saved")` drops a message near the bottom and takes itself away — no barrier, nothing focusable, so
whatever you were doing carries on underneath. It counts down on the frame clock rather than holding a `State`.

### Cursors

```saule
MouseRegion(cursor: "ibeam", child: myField)
```

Nesting resolves the way you'd expect: the innermost region under the pointer wins. Widgets *request* a cursor rather
than setting one, because several are hit at once — move events run depth-first with the front-most sibling first, so
the first request in a frame is the innermost, and the frame loop applies it after routing. The engine call only fires
on a change.

`Button` already asks for `"hand"` and both editors for `"ibeam"`, so this is mostly there for your own widgets.

## Scrolling

```saule
SingleChildScrollView(child: Column(children: rows))
ListView(children: rows, spacing: 2.0)      -- the same thing, plus the Column
```

The wheel scrolls whichever view is under the pointer. A view only *claims* the wheel when the offset actually moved, so
a list nested inside another one hands scrolling back to its parent at either end — the way every real UI behaves.

A scroll view needs a bounded extent on its scroll axis: put it in an
`Expanded`, a `SizedBox`, or anything else that hands down a real height. Unbounded means "shrink-wrap", and there is
then nothing to scroll.

The scrollbar is draggable: grab the thumb, or click the track to page. It only claims presses inside the strip along
the edge, so the rest of the view still gets its clicks, and it draws nothing at all when the content fits.

`ScrollController` is the position, and the only mutable part:

```saule
local list: ScrollController = ScrollController()
...
ListView(children: rows, controller: list)
list.jumpTo(0.0)        -- back to the top
list.progress()         -- 0..1, for a custom indicator
list.atEnd()            -- e.g. to trigger "load more"
```

Pass one in when you need to read or drive the position; otherwise the view keeps its own in element scratch, where it
survives rebuilds. Every child is built up front, so this is for tens of rows, not thousands.

### Fading a subtree

`Opacity(opacity: value, child: panel)` renders the subtree to an offscreen canvas and composites it once, so a group
fades as one image. Per-colour alpha would only fade widgets that happen to take a colour, and would double-darken
wherever they overlap.

It costs one full-size buffer per faded subtree, and the engine has no way to free a canvas — so fade groups rather than
leaves, and don't put one around something that resizes every frame.

## Drawing your own pixels

`CustomPaint` is the escape hatch out of the widget system:

```saule
CustomPaint(
    height: 200.0,
    clip: true,
    painter: fn(x: float, y: float, width: float, height: float)
        Colors.primary.apply()
        Graphics.circle("fill", x + width / 2.0, y + height / 2.0, 40.0)
    end
)
```

The painter is `fn(x, y, width, height)`, called every frame with this widget's absolute position and size, and it draws
with the raw `Graphics` API. It runs inside the tree's paint pass, so it inherits the surrounding clip for free, and its
transform is pushed and popped around it.

### The games pattern

For a simulation, pair `paint` with the per-frame **`tick`** hook and skip the build system entirely:

```saule
export class BouncingBall extends Widget
    fn tick(el: Element, dt: float) -> nil
        -- advance the simulation; state lives in el.data
    end

    fn paint(el: Element, x: float, y: float) -> nil
        -- draw it
    end
end
```

`tick(el, dt)` runs once per frame on every mounted widget, before layout. Painting happens every frame regardless of
rebuilds, so a widget that keeps its state in `el.data` and draws from it costs **zero rebuilds** — the right shape for
a game world with UI layered on top. Call `el.markNeedsBuild()` from `tick`
only when the *widget tree* needs to change, not merely the pixels.
`BouncingBall` in [TestPanel.sau](../Test.sau) is a worked example.

## Images and sprites

```saule
Image(path: "assets/logo.png", height: 64.0, fit: BoxFit.Contain)
Image(path: "assets/sheet.png", frame: Rect.cell(3, 16.0, 16.0, 4))
```

PNG only. `BoxFit` is `None`, `Fill`, `Contain` or `Cover`; with no `width` /
`height` the image takes its natural size, and giving one axis scales the other to match. `frame` picks a cell out of a
spritesheet in image pixels, and
`Rect.cell(index, w, h, columns)` does the grid arithmetic.

Decoding is cached by path in `Images`, so writing `Image(path: ...)` inside a
`build` is safe. A missing file logs once and draws nothing rather than taking the frame down; a file that exists but
isn't a valid PNG is a hard error, because an error inside a native call cannot be caught in Saule.

## Animation

`AnimationController` turns the frame clock into an eased 0..1 value over the existing `Tween` / `EasingTypes`, and
`AnimatedBuilder` drives it:

```saule
local fade: AnimationController = AnimationController(
    duration: 0.4,
    easing: EasingTypes.OutCubic
)
fade.forward()          -- also reverse(), toggle(), stop(), reset()

AnimatedBuilder(
    controller: fade,
    builder: fn(value: float)
        return Container(height: 6.0, width: 200.0 * value, color: Colors.primary)
    end
)
```

The controller is the state, so keep it somewhere that outlives a rebuild (`initState`, or `context.data()`).
`AnimatedBuilder` is stateless and only marks itself dirty on frames where the value actually moved — a finished
animation costs one comparison per frame, not a rebuild. `lerp`, `lerpColor`,
`lerpOffset`, `lerpSize` and `lerpInsets` read the value out. Set `repeats` or
`autoReverse` for a loop.

## Widget reference

**Layout** — `SizedBox`, `Padding`, `Align`, `Center`, `ClipRect`, `Row`,
`Column`, `Flex`, `Expanded`, `Spacer`, `Stack`, `Positioned`, `Wrap`,
`ConstrainedBox`, `AspectRatio`, `FractionallySizedBox`

**Shell** — `Scaffold`, `AppBar`, `Divider`, `VerticalDivider`, `showToast`

**Data** — `GridView`, `DataTable`, `TableColumn`, `TableRow`, `SortOrder`

**Icons** — `Icon`, `IconButton`, `Icons`, `IconFont`

**Overlays** — `showDialog`, `showMenu`, `Tooltip`, `ModalBarrier`, `MenuItem`,
`OverlayHandle`, `Navigator`, `OverlayEntry`, `OverlayManager`

**Scrolling** — `SingleChildScrollView`, `ListView`, `Viewport`, `ScrollBar`,
`ScrollController`

**Painting** — `Container`, `DecoratedBox`, `Text`, `CustomPaint`, `Image`,
`Opacity`

**Controls** — `Button`, `Checkbox`, `Radio`, `Switch`, `Slider`,
`ProgressBar`, `Dropdown`, `Tabs`, `TabBar`, `DateField`

**Input** — `GestureDetector`, `MouseRegion`, `Focus`, `KeyboardListener`,
`TextField`, `TextArea`, `EditableText`, `EditableParagraph`, `TextEditor`,
`LineMap`

**Theme** — `Theme`, `ThemeData`

**Values (cont.)** — `Date`, `LayoutStats`

**Values** — `Size`, `Offset`, `Rect`, `EdgeInsets`, `Alignment`,
`BoxConstraints`, `Color`, `Colors`, `TextStyle`, `PointerEvent`, `KeyEvent`,
`FocusNode`, `BoxFit`, `ScrollAxis`, `TextAlign`

**Animation** — `AnimationController`, `AnimatedBuilder`, `Tween`,
`EasingTypes`

`Container` owns no layout logic: it composes `Padding`, `Align`,
`DecoratedBox`, `SizedBox` and `ClipRect`, and every argument you leave out disappears from the tree.

## Density and resizing

The window is resizable, and the tree is laid out in **logical pixels**: on a 150% display a 200-wide button stays
visually 200 wide instead of shrinking to two-thirds. `runApp` reads `Window.getScale()` every frame, lays out at
`physical / scale`, and scales the root transform to match. Pointer coordinates are converted back down, so hit testing
stays in the same coordinate system as layout.

Text is the part that cannot simply be magnified — a glyph rasterized at 13px and scaled up is blurry. `TextStyle`
builds its font at the *physical* size, reports measurements divided back to logical units, and draws through
`TextStyle.draw`, which maps the anchor into device space and draws with the transform reset. `Icon` does the same with
its font. Everything else scales through the root transform, which is what you want for boxes and lines.

`Display.scale()`, `toLogical()` and `toPhysical()` are there if you need the factor yourself. A `TextStyle` with no
explicit `size` uses the engine's default face, which has no size to rebuild — give type an explicit size if it must
stay sharp on a scaled display.

A resize invalidates every cached layout, so the frame after it re-runs the whole tree and the next one settles back to
zero.

## Notes and limits

- Constants are static **methods**, not fields (`Alignment.center()`,
  `EdgeInsets.all(8.0)`, `Size.zero()`): a static field initializer runs while its own class is still being declared, so
  it cannot reference that class.
  `Colors.primary` is a plain field because `Colors` is a separate class.
- Callbacks are typed `function`, not `fn() -> nil` — Saule rejects `-> nil`
  inside a binding annotation.
- Layout has **relayout boundaries**. An element reuses last frame's size when its constraints are unchanged *and*
  nothing in its subtree was marked dirty, so a settled screen does no layout work at all, and one changed widget
  re-lays out only its own subtree plus the chain to the root — its siblings hit the cache. `LayoutStats.describe()`
  reports the split if you want to check. The contract this rests on: anything that can change layout must mark some
  element dirty. Rebuilds do that automatically; a render widget that mutates layout-affecting state in `el.data` (a
  scroll offset, a drag) has to call
  `el.markNeedsLayout()` itself.
- The focus order is re-derived every frame.
- A `TextStyle` with no `size` binds the engine's default face rather than leaving the current font alone — fonts are
  global state, so inheriting would make a widget's measured size depend on tree-walk order.
- `CrossAxisAlignment.Stretch` needs a bounded cross axis; on an unbounded one it behaves like `Start`.
- The old imperative kit (`View` / `Component` / the retained-mode `Widget`)
  is preserved unused under [legacy/](legacy).

## Not here yet

- **Audio.** Nothing in the engine, and it needs a decision first: `cpal` is portable but pulls ALSA headers into Linux
  builds; raw `winmm` costs no dependency but is Windows-only.
- **A virtualised list.** `ListView` builds every child up front, so it is for tens of rows, not thousands.
- **Drag and drop**, and `Transform` — the latter needs the pointer inverse-transformed on the way down, or a rotated
  widget receives clicks in the wrong place.
- **Selectable static text**, and rich text with mixed styles in one paragraph.
- **Text shaping** — no bidi, no ligatures, no combining marks. Measurement is per-character against the font atlas.
