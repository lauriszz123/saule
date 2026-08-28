# saule-lsp

Language Server Protocol implementation for Saule.

Builds the `saule-lsp` binary, which editors launch as a child process and
drive over stdin/stdout (the conventional LSP transport). It reuses the
compiler front-end to provide diagnostics, hover, goto-definition,
references, document highlight, document symbols, inlay hints, completion,
signature help, and formatting (whole-document and range).

## Modules

| Module       | Responsibility                                            |
|--------------|-----------------------------------------------------------|
| `server`     | LSP backend, document cache, and per-request handlers     |
| `hover`      | Hover information                                         |
| `refs`       | Symbol resolution for goto-definition and find-references |
| `exprty`     | Expression typing rules shared by hover and inlay hints   |
| `syntax`     | Parsing buffers that are not the document itself          |
| `transport`  | Hardening for the stdin side of the connection            |
| `line_index` | Byte-offset ↔ line/column mapping                         |

Built on `tower-lsp` + `tokio`. At startup it calls
`saule_interpreter::init()` so prelude names (`print`, `Math.sqrt`,
`Iterable`, …) aren't flagged as undefined.

## Dependencies

`saule-ast`, `saule-lexer`, `saule-parser`, `saule-semantic`,
`saule-typeck`, `saule-interpreter`, `saule-fmt`.
