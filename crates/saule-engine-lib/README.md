# saule-engine-lib

A small Love2D-style graphics engine compiled as a Saule **native package**,
and the reference consumer of `saule-sdk`.

This crate is **not** linked into the interpreter. It builds as a `cdylib`
(`saule_engine_lib.so` / `.dll` / `.dylib`), is dropped into
`~/.saule/native_packages/`, and is described by a TOML manifest in
`~/.saule/native_manifests/`. At runtime the interpreter loads the shared
library and calls the `extern "C"` symbols named in the manifest.

All ABI plumbing is handled by `saule-sdk`: each module exposes plain safe
functions annotated with `#[saule_export]`, and the package is declared with
`saule_package!`. The `gen-manifest` binary renders the manifest from those
declarations.

## Exposed classes

| Class      | Methods                                                   |
|------------|-----------------------------------------------------------|
| `Window`   | lifecycle, live size, title, position, focus, DPI — see below |
| `Graphics` | the UI drawing surface — see below                        |
| `Keyboard` | key state, per-frame edges, typed text — see below        |
| `Mouse`    | position, buttons, per-frame edges, wheel, cursor — see below |
| `Timer`    | `getTime`, `getDelta`                                     |
| `Clipboard`| `get`, `set`, `hasText`                                   |
| `Util`     | table/function bridge demo helpers                        |

Windowing uses `minifb` (X11-only on Linux, to stay compatible with WSLg).
Everything is drawn by a software rasterizer in `raster.rs` — there is no GPU
involved — with 4× sub-scanline antialiasing and nonzero-winding polygon fills.

## The `Graphics` API

A trimmed take on Love2D's `love.graphics`, keeping the parts that matter for
building user interfaces and dropping the 3D pipeline, meshes, particles and
video.

**Frame** — `clear([r,g,b,a])`, `present()`

**Shapes** — `rectangle(mode, x, y, w, h [, rx, ry])`, `circle(mode, x, y, r
[, segments])`, `ellipse(mode, x, y, rx, ry [, segments])`, `arc(mode, x, y, r,
a1, a2 [, arctype])`, `polygon(mode, points)`, `line(x1, y1, x2, y2)`,
`polyline(points)`, `points(points)`, `point(x, y)`

**Text** — `print(text, x, y)`, `printf(text, x, y, limit [, align])`,
`newFont(size [, path])`, `setNewFont(size [, path])`, `setFont`, `getFont`,
`getFontHeight()`, `getTextWidth(text)`

**State** — `setColor`/`getColor`, `setBackgroundColor`/`getBackgroundColor`,
`setLineWidth`/`getLineWidth`, `setLineStyle`/`getLineStyle`,
`setLineJoin`/`getLineJoin`, `setBlendMode`/`getBlendMode`,
`setDefaultFilter`/`getDefaultFilter`, `reset()`

**Clipping** — `setScissor([x,y,w,h])`, `intersectScissor(x,y,w,h)`,
`getScissor()`

**Canvases & images** — `newCanvas(w, h)`, `newImage(path)`, `setCanvas([handle])`,
`getCanvas()`, `imageSize(handle)`, `draw(handle, x, y [, angle, sx, sy, ox, oy])`,
`drawFrame(handle, fx, fy, fw, fh, x, y [, angle, sx, sy, ox, oy])`

**Transforms** — `push([mode])`, `pop()`, `origin()`, `translate`, `scale`,
`rotate`, `shear`, `applyTransform`, `replaceTransform`, `getStackDepth()`,
`transformPoint`, `inverseTransformPoint`

**Dimensions** — `getWidth`, `getHeight`, `getDimensions`, `getDPIScale`,
`getPixelWidth`, `getPixelHeight`, `getPixelDimensions`

### Differences from Love2D worth knowing

- Drawables are **integer handles**, not objects: `newCanvas` and `newFont`
  return a handle you pass back to `draw`, `setCanvas`, and `setFont`.
- `newFont(size [, path])` takes the size first. With no path you get the
  host's UI typeface, so text works with no assets to ship; font handle `0` is
  that default face.
- Point lists (`polygon`, `polyline`, `points`) take a flat table of
  coordinates — `{x1, y1, x2, y2, ...}` — instead of varargs.
- Scissor rectangles are transformed by the current transform, so clipping
  composes with `translate` the way nested scroll views need. `getScissor`
  reports screen coordinates.
- `push()` saves the transform; `push("all")` saves the whole state.
- `newImage(path)` decodes a PNG (via the pure-Rust `png` crate) into the same
  registry canvases live in, so one handle serves both: `draw` composites it,
  `imageSize` measures it, `drawFrame` picks a spritesheet cell out of it, and
  `setCanvas` can even draw into it. `drawFrame` confines sampling to the cell,
  so a magnified frame never pulls in its neighbours.
- **Not implemented**: shaders (no GPU), formats other than PNG, and
  `newImageFont` / `newText`.

## The `Window` API

**Lifecycle** — `create(width, height [, title, resizable])`, `isOpen()`,
`pollEvents()`, `close()`

### The event queue

`pollEvents()` pumps the OS queue and returns everything that happened since
the last call, in order. This is the primary input API — the level queries on
`Keyboard` and `Mouse` answer only what a queue cannot ("is this held *now*").

Each entry is a positional record, `[kind, payload…]`:

| kind | payload |
| --- | --- |
| `keyPressed`, `keyReleased` | `key`, `shift`, `ctrl`, `alt` |
| `textInput` | `text` |
| `mouseMoved` | `x`, `y`, `dx`, `dy` |
| `mousePressed`, `mouseReleased` | `x`, `y`, `button` |
| `wheelMoved` | `dx`, `dy` (notches, positive away from the user) |
| `resized` | `width`, `height` |
| `focusChanged` | `focused` |
| `closed` | — |

The native ABI has no enum-variant type, so the tagged union is rebuilt on the
Saule side. `examples/native-package/keyboard.sau` shows the decoder pattern,
and UIKit ships one in its `Events.sau`:

```saule
for ev: Event in pollEvents() do
	match ev
		case Event.KeyPressed(key, _, ctrl, _) when ctrl then shortcut(key)
		case Event.TextInput(text) then insert(text)
		case _ then nil
	end
end
```

**Ordering.** Keyboard messages keep true arrival order — they are recorded in
the backend's input callback as each OS message lands, and modifiers are
captured at that moment rather than read back at the end of the frame. Mouse
and window events are derived per frame, since the backend offers no callback
for them, so within one frame the order is: window changes, mouse motion, mouse
buttons, wheel, then the keyboard log. Motion before buttons is the one that
matters — a click is always delivered against an up-to-date pointer position.

Ignoring the return value is fine: `pollEvents()` still pumps the queue, which
is all a game loop reading held keys needs.

**Geometry** — `getSize()`, `getPosition()`, `setPosition(x, y)`

**Chrome** — `setTitle(title)`, `setTopmost(topmost)`, `isFocused()`

**Density** — `getScale()`

Windows are resizable by default. The framebuffer is reallocated to match
whenever the window changes size, so `getSize` reports live dimensions and
drawing never lands in a stale buffer; the scissor is reset at the same moment,
since a clip from the old size could sit entirely outside the new one.

`getScale()` returns the display's scale factor — `1.0` at 96 DPI, `1.5` at
150%. minifb has no DPI query, so the engine asks the OS through the native
window handle, and declares the process **per-monitor DPI aware** before the
first window opens. Without that declaration Windows reports 96 DPI and quietly
magnifies the window's pixels, which produces a blurry UI with no way to detect
it. The scale is re-read every `pollEvents`, so moving a window between monitors
of different density is picked up.

Only Windows reports a real figure today; the other platforms return `1.0`
rather than guessing.

## The `Mouse` API

Motion, buttons and the wheel arrive as events (see above); what stays here is
the state a queue cannot answer.

**Position** — `getPos()`, where the pointer is right now.

**Buttons** — `isDown(button)`, whether it is held right now. `1` = left,
`2` = right, `3` = middle.

**Cursor** — `setCursor(style)`, `setVisible(visible)`. Styles are `"arrow"`,
`"ibeam"`, `"crosshair"`, `"hand"`, `"grab"`, `"resizeleftright"`,
`"resizeupdown"`, `"resizeall"`; an unknown name is an error rather than a
silent no-op.

Input is sampled after *both* of minifb's message pumps — `update` in
`pollEvents` and the one inside `update_with_buffer` in `present` — and the
extra edges are carried into the next frame. Each pump begins by resetting
minifb's own scroll delta and key tracking, so anything that arrived during the
present half of a frame used to be wiped before Saule could read it: roughly
half of all wheel notches and any keystroke short enough to start and finish in
that window. The keyboard does the same thing for the same reason.

## The `Clipboard` API

`get()`, `set(text)`, `hasText()` — plain text only. `get()` returns `""` when
the clipboard is empty or holds something that isn't text, so a paste handler
needs no error handling.

Backed by `arboard` with default features off, which keeps it pure Rust: x11rb
on Linux, `windows-sys` on Windows. The connection is kept alive for the life of
the process because on X11 the clipboard is *owned* by a live connection —
dropping it would take the copied text with it.

## The `Keyboard` API

Love2D splits keyboard input between level queries and callbacks
(`love.keypressed`, `love.keyreleased`, `love.textinput`). Saule owns the loop
here, so the callbacks become `KeyPressed` / `KeyReleased` / `TextInput` events
in the queue, and what remains on `Keyboard` is the level half.

**Held keys** — `isDown(key)`, `isAnyDown({key, ...})`, `getKeysDown()`

**Key repeat** — `setKeyRepeat(enable)`, `hasKeyRepeat()`

**Text gate** — `setTextInput(enable)`, `hasTextInput()`. Off means no
`TextInput` events are produced; disabling it mid-frame also drops text already
typed this frame, so a handler never sees input the app has just declined.

Key names are Love2D's `KeyConstant` strings — `"a"`, `"space"`, `"lshift"`,
`"return"`, `"kp0"`, `"/"` — so code reads the same as its Love2D equivalent.
Unrecognised names report as "not down" rather than erroring. See
`examples/native-package/keyboard.sau` for a text field and WASD movement built
on the whole surface.

### Differences from Love2D worth knowing

- The callbacks are a queue, not per-frame flags. A key tapped and released
  between two polls still reports both edges, and two taps of the same key in
  one frame stay two events — neither survives a level-diffing API.
- `isAnyDown({"lshift", "rshift"})` replaces Love2D's variadic
  `isDown(key, ...)`, since native calls take a fixed argument list.
- `TextInput` carries text the way `love.textinput` would deliver it — layout-
  and modifier-aware, so shift+`a` arrives as `"A"`. A run of characters
  coalesces into one event, capped at 4 KiB.
- `setKeyRepeat(true)` makes a held key keep emitting `KeyPressed` after a
  0.25 s delay, then every 0.05 s. Repeats are synthesised by the engine, so
  they land after the frame's real messages.
- **Not implemented**: scancodes (`isScancodeDown`, `getScancodeFromKey`,
  `getKeyFromScancode`). The windowing backend reports keys already mapped
  through the OS layout, so there is no honest physical-position code to hand
  back.

## Building & installing

```sh
cargo build -p saule-engine-lib --release
```

Then install with the script for your platform:

| Platform    | Script                        | Regenerates the manifest? |
|-------------|-------------------------------|---------------------------|
| Windows     | `scripts\install_windows.ps1` | yes                       |
| Linux / WSL | `scripts/install_wsl.sh`      | no — copies the checked-in one |
| macOS       | `scripts/install_mac.sh`      | no — copies the checked-in one |

The Unix scripts install `target/release/engine.toml`, which `build.rs` copies
from the crate-root `engine.toml`. After changing any `#[saule_export]`
signature, refresh that file first so the two cannot drift:

```sh
cargo run --release -p saule-engine-lib --bin gen-manifest -- crates/saule-engine-lib/engine.toml
```

Or copy the library plus the generated `target/release/engine.toml` into
`~/.saule/native_packages/` and `~/.saule/native_manifests/` manually.

See `examples/native-package/` for `.sau` programs that import this package.
