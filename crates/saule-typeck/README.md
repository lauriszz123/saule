# saule-typeck

Static type checks performed between parsing and evaluation.

- **Field initialization** — every non-nullable instance field of a class
  with a constructor must be assigned in that constructor's body. Fields
  with defaults and nullable fields (`name: string?`) are exempt.
- **Null safety** — a nullable value cannot be silently assigned to a
  non-nullable binding, and members cannot be read off a nullable receiver
  without `?.`, `!`, or a prior nil-guard. A lightweight flow-narrowing
  pass treats `if x != nil then ... end` as proving `x` non-nullable inside
  the block.

Type inference is intentionally partial: when the checker cannot prove a
type (calls, members on unknown classes) it skips the check rather than
emit a false positive. Native-function signatures are registered through
`sigs` by embedders.

## Place in the pipeline

```text
lexer → parser → semantic → [typeck] → interpreter
```

Depends on `saule-ast`. Errors are reported as `TypeCheckError`.
