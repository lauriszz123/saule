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
- `format_module_with_options` — as above plus an explicit `FmtOptions`.

Used by `saule fmt` in the CLI and by `saule-lsp` for
`textDocument/formatting`.

## `FmtOptions`

| Field | Default | Meaning |
|---|---|---|
| `indent_width` | `2` | Columns per indent level (clamped to `1..=16`). |
| `use_tabs` | `false` | Indent with hard tabs; `indent_width` still gives the display width used for column arithmetic. |
| `max_width` | `100` | Soft target — breakable constructs go multi-line past it. |

`saule-lsp` maps the editor's LSP `FormattingOptions` (`tabSize` /
`insertSpaces`) onto these, which is what makes an IDE's Code Style page take
effect. **Keep the defaults in step with the IntelliJ plugin's
`SauleCodeStyleSettingsProvider.customizeDefaults`** — if they disagree, the
editor silently reformats to something other than what `saule fmt` produces.

## Layout rules

- **Blank lines** collapse to at most one, and a blank line the author left
  after a comment is preserved: a comment written directly above a statement
  captions it and stays attached, one separated by a blank line reads as a
  section header and keeps its gap.
- **Breakable constructs** — table literals, `when(...)` pipelines, call
  argument lists, and parameter lists — render inline when they fit within
  `max_width` from the current column, and one-item-per-line otherwise. Table
  literals additionally stay multi-line if the author already broke them.
- **Trailing commas** are emitted for table literals only. The parser requires
  an argument after every comma in a call or parameter list, so a trailing one
  there would make the formatter's own output unparseable.
- **Not wrapped**: long boolean conditions and binary-operator chains, and
  anything unbreakable such as a long string literal.

## Dependencies

`saule-ast`, `saule-lexer`, `saule-parser`.
