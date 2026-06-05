# saule-export-macro

Procedural macro behind `#[saule_export]`, re-exported by `saule-sdk`.
Authors normally depend on `saule-sdk`, not this crate directly.

Annotate a plain, safe Rust function with the owning class and method name:

```rust
#[saule_export(class = "Window", name = "create")]
fn window_create(width: i64, height: i64, title: Option<String>) -> Result<(), String> {
    /* ... */
    Ok(())
}
```

From the signature above the macro:

- **infers the Saule type** — `i64 → integer`, `f64 → float`,
  `bool → boolean`, `String → string`, `Option<T> → T?`, `() → nil`, and
  `Result<T, E>` marks the export as fallible;
- **generates the `extern "C"` shim** — null / arity checks, per-argument
  decoding, return marshalling, and error surfacing;
- **registers the method** in the manifest via `inventory`, and emits a
  `#[used]` anchor so the registration survives static linking.

The original function is left intact and unit-testable. Generated code
references `::saule_sdk::__private::*`, so the annotated crate must depend on
`saule-sdk`.

## Dependencies

`syn`, `quote`, `proc-macro2`.
