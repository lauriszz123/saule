# saule-runtime

What a running Saule program is made of: its values, the standard library,
native packages, and the operations decided by the value in hand rather than
at compile time. `saule-vm` compiles programs to bytecode and runs them on
this crate.

```text
source → lexer → parser → semantic → typeck → saule-vm (compile → run on saule-runtime)
```

`analyze_and_check` runs the static half of that pipeline (semantic analysis,
then type-checking), stopping at the first error. `init` wires the standard
library's signatures and names into the checker; every entry point calls it.

## Module layout

| Module       | Responsibility                                          |
|--------------|---------------------------------------------------------|
| `value`      | Runtime `Value` enum and the objects behind it          |
| `prelude`    | The names every program starts with (`Prelude`)         |
| `stdlib`     | Standard library, installed into the prelude            |
| `ops`        | Operators on values the compiler could not type         |
| `cast`       | `x as T`: the checked downcast and the conversion       |
| `members`    | `obj.name` and `obj[index]` by name, read and written   |
| `call`       | Calling a value or a method by name; the depth guard    |
| `module`     | Where an `import` points; the typecheck seed            |
| `native_packages` / `dynamic_packages` | Native packages, built in and `.so`/`.dll` |
| `error`      | `RuntimeError` (miette-aware)                           |

Runtime errors (division by zero, force-unwrap of `nil`, uncaught `throw`,
I/O failures, …) are kept disjoint from compile-time diagnostics.

## Dependencies

`saule-ast`, `saule-lexer`, `saule-parser`, `saule-semantic`,
`saule-typeck`, `saule-native-abi`.
