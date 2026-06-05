# saule-ast

Abstract syntax tree for the Saule language.

Defines the node types every other compiler stage shares — `Type`, `Expr`,
`Stmt`, `Decl` and their supporting enums — plus the generic
[`Spanned<T>`] wrapper that pairs each node with the byte range it occupies
in the source text. The byte ranges feed `miette` diagnostics across the
lexer, parser, type-checker and interpreter via the canonical
`to_source_span` helper.

All node types are re-exported flat, so downstream crates write
`saule_ast::{Type, Expr, Stmt, ...}` without caring about the internal
module split (`types`, `expr`, `stmt`, `decl`).

## Place in the pipeline

```text
lexer → parser → [AST] → semantic → typeck → interpreter
```

This crate has no dependencies on other Saule crates; everything else
depends on it.
