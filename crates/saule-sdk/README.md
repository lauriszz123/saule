# saule-sdk

The authoring SDK for Saule native packages. Write safe Rust functions; the
SDK does all the ABI heavy lifting — shim generation, argument decoding,
return marshalling, signature inference, and manifest generation.

```rust
use saule_sdk::prelude::*;

saule_sdk::saule_package! {
    name = "engine",
    version = "0.1.0",
    binary = ["mypkg.so", "mypkg.dll", "mypkg.dylib"],
    classes {
        Window = "Window management.",
    }
}

#[saule_export(class = "Window", name = "create")]
fn window_create(width: i64, height: i64, title: Option<String>) -> Result<(), String> {
    Ok(())
}
```

## What it provides

| Item | Role |
|------|------|
| `#[saule_export]` | Re-export of `saule-export-macro`; turns a safe fn into an ABI export + manifest entry |
| `saule_package!`  | Declares package name, version, binaries and classes |
| `FromSaule` / `IntoSaule` (`convert`) | Decode/encode `CValue` for `i64`, `f64`, `bool`, `String`, `Option<T>`, `&str`, `()` |
| `manifest::render` | Renders the package's TOML manifest from the registered exports |

The signature shown above is inferred as
`fn(width: integer, height: integer, title: string?) -> nil`.

## Dependencies

`saule-native-abi`, `saule-export-macro`, `inventory`.

See `crates/saule-engine-lib` for a complete reference consumer.
