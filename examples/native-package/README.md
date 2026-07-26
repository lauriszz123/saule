# Native packages — dynamically-loaded engine modules

This folder demonstrates Saule's **native package** system: compile a Rust
library to a shared object, describe it with a TOML manifest, and `import` it
from Saule with full type-checking and LSP support — **no interpreter rebuild
required**.

## How it works

```
~/.saule/
  native_manifests/engine.toml          ← describes exports + symbol names
  native_packages/saule_engine_lib.dll  ← the compiled code (.dll/.so/.dylib)
```

1. At startup the interpreter scans `~/.saule/native_manifests/`, parses every
   manifest, and registers each method's type signature. `Graphics.circle(...)`
   now type-checks **before** any binary is loaded.
2. The first time your code runs `import Graphics from "engine"`, the
   interpreter loads the shared library named in the manifest (preferring the
   one matching your OS) and binds each export to its native symbol.
3. Calls cross a small, frozen C ABI ([`saule-native-abi`](../../crates/saule-native-abi))
   — Saule values in, a Saule value out.

The contract is entirely declarative: the manifest says which symbols exist and
what their Saule signatures are. There is **no** `get_package()` entry point to
implement — just plain `extern "C"` functions.

The manifest itself is **generated from the code**: each exported function
carries a `#[saule_export(class = ..., name = ..., sig = ...)]` attribute, and a
small `gen-manifest` binary (built alongside the library) walks those
declarations to emit `engine.toml`. There is no hand-maintained manifest to
keep in sync.

## Build & install

> **Toolchain.** The build works on both Windows and Linux/WSL:
> - **Linux / WSL** — the default `stable-x86_64-unknown-linux-gnu` builds
>   everything. minifb loads X11/Wayland via `dlopen`, so no `-dev` headers are
>   needed. (In WSL, make sure you use the rustup `cargo`, not the old apt one —
>   `source ~/.cargo/env`. The helper `scripts/build_wsl.sh` does this and uses a
>   separate target dir so Linux and Windows artifacts don't collide.)
> - **Windows** — use the **MSVC** toolchain; the GNU toolchain can't link the
>   `cdylib`/`libloading` (missing `dlltool.exe`). Pin it for this folder once:
>   `rustup override set stable-x86_64-pc-windows-msvc`.

```sh
# 1. Build the example engine as a shared library (also builds gen-manifest)
cargo build -p saule-engine-lib --release

# 2. Create the package directories
#    (Windows PowerShell)
mkdir $env:USERPROFILE\.saule\native_packages -Force
mkdir $env:USERPROFILE\.saule\native_manifests -Force

# 3. Copy the compiled binary (pick your platform's file)
#    Windows:
copy target\release\saule_engine_lib.dll   $env:USERPROFILE\.saule\native_packages\
#    Linux:   target/release/libsaule_engine_lib.so   → ~/.saule/native_packages/saule_engine_lib.so
#    macOS:   target/release/libsaule_engine_lib.dylib → ~/.saule/native_packages/saule_engine_lib.dylib

# 4. Generate and install the manifest (it is emitted from the code, not
#    checked in). Pass an output path, or run it with none to write
#    engine.toml next to the binary.
.\target\release\gen-manifest.exe $env:USERPROFILE\.saule\native_manifests\engine.toml
```

> On Linux/WSL, `scripts/install_wsl.sh` does steps 3–4 for you: it runs
> `gen-manifest` and copies both the `lib`-stripped `.so` and the manifest into
> `~/.saule/`.

> The manifest's `binary = "..."` field lists the candidate filenames. On Linux
> and macOS, Cargo prefixes the output with `lib`, so either rename the file to
> match the manifest or update the manifest to the `lib`-prefixed name.

## Run

```sh
saule examples/native-package/demo.sau
```

## Import forms

```saule
import Graphics from "engine"                 -- single class
import Graphics, Window, Timer from "engine"  -- several classes
import * from "engine"                        -- everything the manifest exports
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

Input is read inside that loop, between `pollEvents` and `present`. See
[keyboard.sau](keyboard.sau) for the `Keyboard` surface — held keys
(`isDown`, `isAnyDown`), per-frame edges (`wasPressed`, `wasReleased`, which
stand in for Love2D's `keypressed` / `keyreleased` callbacks), and typed text
(`getTextInput`, standing in for `love.textinput`).

## Adding your own functions

1. Add an `extern "C"` symbol in `crates/saule-engine-lib/src/` following the
   `(args, argc, out) -> i32` ABI (use the `Args` helper to read arguments).
2. Annotate it with `#[saule_export(class = "<Class>", name = "<method>", sig =
   "fn(...) -> ...")]`. The manifest entry is generated from this — no TOML to
   edit. (For a brand-new class, also add an `ExportedClass` registration in
   `crates/saule-engine-lib/src/lib.rs`.)
3. Rebuild and reinstall (`scripts/install_wsl.sh`, or rerun `gen-manifest`).
   No interpreter changes.

## Note on the toolchain (Windows)

Loading shared libraries pulls in the `libloading` crate, which on Windows
requires the **MSVC** toolchain (the GNU toolchain needs MinGW's `dlltool`).
The workspace `rust-toolchain.toml` pins MSVC so this works out of the box.
