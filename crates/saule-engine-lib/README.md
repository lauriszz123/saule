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
| `Window`   | `create`, `isOpen`, `pollEvents`, `close`, `getSize`      |
| `Graphics` | the UI drawing surface — see below                        |
| `Keyboard` | `isDown`                                                  |
| `Mouse`    | `getPos`, `isDown`                                        |
| `Timer`    | `getTime`, `getDelta`                                     |
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

**Canvases** — `newCanvas(w, h)`, `setCanvas([handle])`, `getCanvas()`,
`draw(canvas, x, y [, angle, sx, sy, ox, oy])`

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
- **Not implemented**: shaders (no GPU), and image loading — `newImage`,
  `newQuad`, `newImageFont`, and `newText` would all need an image decoder.
  Canvases cover offscreen rendering and caching in the meantime.

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
