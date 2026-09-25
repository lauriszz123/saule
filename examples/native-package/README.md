# Native packages — dynamically-loaded Rust modules

This folder demonstrates Saule's **native package** system: compile a Rust
library to a shared object, drop it into `~/.saule/native_packages/`, and
`import` it from Saule with full type-checking and LSP support — **no
interpreter rebuild, and no manifest**.

The `.sau` programs here import **Shine2D**, the Love2D-style engine that used
to live in this repository as `crates/saule-engine-lib`. It now has a
repository of its own — <https://github.com/lauriszz123/saule-shine> — which is
the point: a native package is built and installed from outside the language,
against the published SDK, exactly as anyone else's would be. Install it first
and these examples run.

## How it works

```
~/.saule/
  native_packages/libsaule_shine.dylib        ← the whole package (.dll/.so/.dylib)
```

A package is one file. `saule-sdk`'s macros compile a description of the
package **into the library**: every class, every method's Saule signature and
exported symbol, and your `///` doc comments.

1. At startup the interpreter scans `~/.saule/native_packages/` and reads each
   library's description **out of the file, without loading it**. Each
   method's signature is registered, so `Graphics.circle(...)` type-checks —
   and the editor completes it and shows its docs — before any of the
   package's code runs.
2. When a program reaches `import Graphics from "shine"`, the interpreter
   loads the library, checks that it was built against the same native ABI,
   and binds each export to its symbol.
3. Calls cross a small, frozen C ABI ([`saule-native-abi`](../../crates/saule-native-abi))
   — Saule values in, a Saule value out.

A library that cannot be used — built for another ABI, not a Saule package,
or installed the old two-file way — is reported at the `import` that names
it, with the reason.

Beyond namespaces of functions like `Graphics`, a package can define real
classes (`#[saule_class]` + `#[saule_methods]`), whose objects Saule programs
hold, call methods on, and read properties of, and enums (`#[saule_enum]`).
See [`saule-sdk`'s README](../../crates/saule-sdk/README.md).

### Callback parameters

Saule has no bare `function` type — every function type spells out its
parameters and return, so a callback a package accepts has to say what it will
be called with. An `SFunction` parameter is only a handle, so the signature is
declared in the attribute, keyed by the Rust parameter's name:

```rust
#[saule_export(class = "Signal", name = "onEach", sig(f = "fn(T) -> nil"))]
fn signal_on_each(t: STable<T>, f: SFunction) -> Result<(), String> { /* ... */ }
```

That renders as `fn<T>(t: table<T>, f: fn(T) -> nil) -> nil` in the package's
metadata, which is what the type checker and the LSP check call sites against — so
`Signal.onEach(nums, x => println(x))` type-checks and the lambda's parameter
is inferred. Omitting `sig(...)` for an `SFunction` is a compile error rather
than a silently untyped parameter. `sig(return = "...")` types an `SFunction`
in return position, and `Option<SFunction>` parenthesises its declared
signature — `(fn(T) -> nil)?` — because `?` would otherwise bind to the return
type.

## Build & install

> **Toolchain.** The build works on both Windows and Linux/WSL:
> - **Linux / WSL** — the default `stable-x86_64-unknown-linux-gnu` builds
>   everything. minifb loads X11/Wayland via `dlopen`, so no `-dev` headers are
>   needed. (In WSL, make sure you use the rustup `cargo`, not the old apt one —
>   `source ~/.cargo/env`.)
> - **Windows** — use the **MSVC** toolchain; the GNU toolchain can't link the
>   `cdylib`/`libloading` (missing `dlltool.exe`). Pin it for this folder once:
>   `rustup override set stable-x86_64-pc-windows-msvc`.

```sh
# 1. Clone and build Shine2D — a separate repository, not part of this one
git clone https://github.com/lauriszz123/saule-shine
cd saule-shine && cargo build --release

# 2. Copy the library into the packages directory (any file name works —
#    Saule reads the package's real name out of the file)
#    Linux:   cp target/release/libsaule_shine.so    ~/.saule/native_packages/
#    macOS:   cp target/release/libsaule_shine.dylib ~/.saule/native_packages/
#    Windows (PowerShell):
mkdir $env:USERPROFILE\.saule\native_packages -Force
copy target\release\saule_shine.dll $env:USERPROFILE\.saule\native_packages\
```

> **macOS.** Apple's linker can leave a release build's symbol string table
> misaligned, and macOS then refuses to load it. Run
> `python3 scripts/align_macho_strtab.py <the .dylib>` from this repository
> afterwards; it is a no-op when the build came out fine. Shine2D's README has
> the details.

## Run

```sh
saule examples/native-package/demo.sau
```

## Import forms

```saule
import Graphics from "shine"                 -- single class
import Graphics, Window, Timer from "shine"  -- several classes
import * from "shine"                        -- everything the package exports
```

## Game loop

Our C ABI only marshals primitives (no function callbacks), so the loop is
**driven from Saule** rather than the love2d callback style (`love.update` /
`love.draw` called *by* the runtime). Saule owns the `while`; each iteration
calls native functions to pump events, read the frame delta, and draw:

```saule
Window.create(800, 600)
while Window.isOpen() do
    Window.pollEvents()              -- input / OS events
    local dt: float = Timer.getDelta()  -- seconds since last frame
    -- update game state with dt ...
    Graphics.clear(0.1, 0.1, 0.12)   -- begin frame
    Graphics.circle("fill", x, y, 32.0)
    Graphics.present()               -- end frame / swap buffers
end
```

See [gameloop.sau](gameloop.sau) for a complete moving-circle example. It calls
`Window.runFor(120)` so the headless stub terminates after 120 frames — a real
windowed build would drop that and close on the OS quit event.

| Function | Purpose |
| --- | --- |
| `Window.isOpen() -> boolean` | Loop condition. |
| `Window.pollEvents() -> nil` | Pump OS/input events once per frame. |
| `Window.close() -> nil` | End the loop programmatically. |
| `Window.runFor(frames: integer) -> nil` | Headless: auto-close after N frames. |
| `Timer.getDelta() -> float` | Seconds since the previous frame. |
| `Timer.getTime() -> float` | Seconds since `Window.create`. |
| `Graphics.clear(r, g, b) -> nil` | Begin a frame (clear to colour). |
| `Graphics.present() -> nil` | End a frame (swap buffers). |

Input arrives from `pollEvents` itself, which returns everything that happened
since the last frame in order — key presses and releases, typed text, mouse
motion, buttons, the wheel, and window changes. Each entry is a positional
record `[kind, payload…]`; the engine README has the full table.

See [keyboard.sau](keyboard.sau) for the pattern: decode the records into an
`enum` once, then `match` on it. That is how the callbacks Love2D would give you
(`keypressed`, `keyreleased`, `textinput`) are spelled here, and unlike
per-frame flags it keeps two taps of the same key inside one frame apart.

What stays a query is the state a queue cannot answer: `Keyboard.isDown`,
`isAnyDown`, `getKeysDown`, and `Mouse.getPos` / `isDown` all report what is
true *right now*. `Mouse.setCursor` / `setVisible` set the cursor image.

Images are PNG files loaded with `Graphics.newImage(path)`. They share the
handle registry with canvases, so one handle works everywhere:

| Function | Purpose |
| --- | --- |
| `Graphics.newImage(path: string) -> integer` | Decode a PNG; returns a handle. |
| `Graphics.imageSize(handle) -> (integer, integer)` | Pixel dimensions of an image or canvas. |
| `Graphics.draw(handle, x, y [, angle, sx, sy, ox, oy])` | Composite the whole thing. |
| `Graphics.drawFrame(handle, fx, fy, fw, fh, x, y [, angle, sx, sy, ox, oy])` | One spritesheet cell; sampling stays inside the cell, so frames never bleed. |

See [images.sau](images.sau) for all of it together — a spritesheet animation,
wheel-driven zoom, mouse edges, and a canvas used as a pre-rendered backdrop.

## Adding your own functions

In Shine2D's `src/`, or in a package of your own — the steps are the same, and
neither needs a checkout of the language:

1. Write a plain, safe Rust function.
2. Annotate it with `#[saule_export(class = "<Class>", name = "<method>")]`.
   The Saule signature is inferred from the Rust types, and the function's
   `///` comment becomes its hover text — both compiled into the library.
   (To document a brand-new class, add it to the `classes { … }` list in
   `saule_package!` in the crate root.)
3. `cargo build --release` and copy the library over the installed one. No
   interpreter changes, and nothing to regenerate.

For a class whose *objects* Saule programs hold — a texture, a socket — use
`#[saule_class]` and `#[saule_methods]` instead. See
[the SDK's README](../../crates/saule-sdk/README.md) for the whole surface, and
[`crates/saule-native-fixture`](../../crates/saule-native-fixture) for a small
package that exercises every part of it.

## Note on the toolchain (Windows)

Loading shared libraries pulls in the `libloading` crate, which on Windows
requires the **MSVC** toolchain (the GNU toolchain needs MinGW's `dlltool`).
The workspace `rust-toolchain.toml` pins MSVC so this works out of the box.
