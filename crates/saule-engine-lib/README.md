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

| Class      | Methods                                            |
|------------|----------------------------------------------------|
| `Window`   | `create`, `isOpen`, `pollEvents`, `close`          |
| `Graphics` | `setColor`, `circle`, `rectangle`, `clear`, `present` |
| `Timer`    | `getTime`, `getDelta`                              |

Windowing uses `minifb` (X11-only on Linux, to stay compatible with WSLg).

## Building & installing

```sh
cargo build -p saule-engine-lib --release
# then install with scripts/install_wsl.sh (Linux/WSL), or copy the
# library + generated engine.toml into ~/.saule/ manually.
```

See `examples/native-package/` for `.sau` programs that import this package.
