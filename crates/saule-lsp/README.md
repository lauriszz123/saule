# saule-lsp

Language Server Protocol implementation for Saule.

Builds the `saule-lsp` binary, which editors launch as a child process and
drive over stdin/stdout (the conventional LSP transport). It reuses the
compiler front-end to provide diagnostics, hover, and references.

## Modules

| Module       | Responsibility                          |
|--------------|-----------------------------------------|
| `server`     | LSP backend / request dispatch          |
| `hover`      | Hover information                       |
| `refs`       | Find references                         |
| `workspace`  | Open-document state and re-analysis     |
| `line_index` | Byte-offset ↔ line/column mapping       |

Built on `tower-lsp` + `tokio`. At startup it calls
`saule_interpreter::init()` so prelude names (`print`, `Math.sqrt`,
`Iterable`, …) aren't flagged as undefined.

## Dependencies

`saule-ast`, `saule-lexer`, `saule-parser`, `saule-semantic`,
`saule-typeck`, `saule-interpreter`, `saule-fmt`.
