# saule-interpreter

Tree-walking interpreter for Saule, and the embedding entry point that ties
the whole front-end together.

```text
source → lexer → parser → semantic → typeck → run / run_in
```

`check_and_run` walks the full pipeline (semantic analysis, then
type-checking, then evaluation), skipping later stages when an earlier one
reports errors. Lower-level entry points (`run`, `run_in`,
`call_class_static_method`) let embedders such as the CLI and LSP reuse
pieces directly.

## Module layout

| Module       | Responsibility                                   |
|--------------|--------------------------------------------------|
| `value`      | Runtime `Value` enum and `NativeFn`              |
| `env`        | Lexical scopes (`Environment`)                   |
| `stdlib`     | Standard library installed into the prelude      |
| `eval`       | Statement & expression evaluation                |
| `module`     | `import` loader — runs the full pipeline          |
| `native_packages` / `dynamic_packages` | Loading native `.so`/`.dll` packages |
| `error`      | `RuntimeError` (miette-aware)                     |

Runtime errors (division by zero, force-unwrap of `nil`, uncaught `throw`,
I/O failures, …) are kept disjoint from compile-time diagnostics.

## Dependencies

`saule-ast`, `saule-lexer`, `saule-parser`, `saule-semantic`,
`saule-typeck`, `saule-native-abi`.
