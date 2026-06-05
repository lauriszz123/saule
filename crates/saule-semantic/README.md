# saule-semantic

Semantic analysis — the pass that runs after parsing and before
type-checking. It catches everything decidable from program structure
alone, without inferring types:

- **Registry build** — collects all declared classes, interfaces and enums
  into one shared source of truth (`registry`) that later passes consult.
- **Definite assignment** — every non-nullable instance field of a class
  with a constructor must be assigned `self.field = ...` in that
  constructor.
- **Control-flow validity** — `break` / `continue` only inside loops;
  `return` only inside functions.

`analyze` is the gate that runs first once a `Module` is in hand; a
non-empty `SemanticError` list means type-checking should be skipped.

## Place in the pipeline

```text
lexer → parser → [semantic] → typeck → interpreter
```

Depends on `saule-ast`. Also exposes `builtins`, `prelude` and `registry`
for embedders.
