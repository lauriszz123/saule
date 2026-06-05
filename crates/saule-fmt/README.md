# saule-fmt

Source pretty-printer for Saule.

Walks a parsed `saule_ast::Module` and renders it back to canonical source:
2-space indent, one statement per line, blank line between top-level
declarations.

- `format_module` — fast path; discards comments (the AST never sees them).
- `format_module_with_comments` — round-trips comments. Pair it with the
  lexer's `tokenize_with_trivia`: extract each `LineComment` / `BlockComment`
  into a `Comment` and pass them in. Interleaving is best-effort but handles
  leading, trailing same-line, and end-of-block comments, and preserves
  blank-line groupings.

Used by `saule fmt` in the CLI.

## Dependencies

`saule-ast`, `saule-lexer`, `saule-parser`.
