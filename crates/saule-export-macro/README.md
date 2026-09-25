# saule-export-macro

The procedural macros behind `saule-sdk`: `#[saule_export]`,
`saule_package!`, `#[saule_class]`, `#[saule_methods]` and `#[saule_enum]`.
Authors depend on `saule-sdk`, which re-exports them, not on this crate.

For each declaration the macros:

- **infer the Saule type** from the Rust one — see the mapping in
  `saule-sdk`'s README;
- **generate the `extern "C"` shim** — arity checks, per-argument decoding,
  borrowing objects for the call, return and error marshalling, and turning
  a panic into a Saule error;
- **compile a metadata record** into the library: an exported static holding
  the member's name, signature, symbol and doc comment as TOML. The
  interpreter reads these out of the library file without loading it; the
  format is defined in `saule-native-abi` ("Package metadata").

Because the record is generated from the same declaration as the shim, a
package's description cannot drift from its code, and there is nothing to
regenerate or keep in sync.

Generated code refers to `::saule_sdk`, so the annotated crate must depend
on `saule-sdk`.

## Dependencies

`syn`, `quote`, `proc-macro2`, and `saule-native-abi` for the record format
and the ABI version a package declares.
