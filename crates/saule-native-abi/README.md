# saule-native-abi

The stable C-ABI boundary shared between the Saule interpreter and any
dynamically-loaded native package (`.so` / `.dll` / `.dylib`).

The interpreter never links a native package directly — it loads the shared
library at runtime (via `libloading`) and calls exported `extern "C"`
symbols named in a TOML manifest. This crate is the *only* contract both
sides agree on, so its layout is **frozen**: changing `CValue` is a breaking
ABI change.

## Calling convention

Every exported native function has this exact signature:

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn some_symbol(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 { /* ... */ }
```

- `args` / `argc` — positional arguments as `CValue`. String payloads are
  valid only for the duration of the call.
- `out` — write the single return value here (`CValue::nil` for `nil`).
- return code — `0` success; non-zero means failure, with an error message
  written into `out` via `CValue::error`.

Strings cross as `(ptr, len)` UTF-8 pairs; the producer keeps the bytes
alive until the consumer copies them.

> Most authors never touch this crate directly — `saule-sdk` generates the
> shims for you. This is the low-level foundation underneath it.

No dependencies on other Saule crates.
