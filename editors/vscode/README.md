# Saule for VS Code

Full language support for `.sau` files: a TextMate grammar for syntax
highlighting **plus** a Language Server Protocol client that talks to
`saule-lsp`.

## Features

Powered by the `saule-lsp` server (`crates/saule-lsp`):

- **Diagnostics** — lex, parse, semantic, and type errors, live on every
  edit.
- **Hover** — types and signatures for locals, functions, methods,
  classes, enums, and stdlib/native members (with generic substitution,
  so `Util.filter(table<integer>)` shows `table<integer>`).
- **Go-to-definition** and **find-all-references**.
- **Document highlights** and **document symbols** (outline / breadcrumbs).
- **Inlay hints** — inferred local types and parameter-name labels.
- **Signature help** — parameter popups while typing call arguments.
- **Formatting** — full-document and range formatting.

## Build the language server (one time)

From the repo root:

```powershell
cargo build --release -p saule-lsp
```

The extension auto-discovers the binary at
`<workspaceFolder>/target/release/saule-lsp` (then `target/debug`, then
`saule-lsp` on your `PATH`). No `PATH` setup needed when you work inside
the Saule repo.

## Install the extension

```powershell
# from the repo root
cd editors\vscode
npm install
npm run compile

# then either press F5 / "Run Extension" from Run & Debug,
# or package and install:
npm install -g @vscode/vsce
vsce package
code --install-extension saule-26.1.0.vsix
```

For zero-build syntax-only dev, copy the `editors/vscode` folder into
`%USERPROFILE%\.vscode\extensions\saule\` and reload — but the LSP
features need the compiled `out/extension.js` (`npm run compile`).

## Settings

| Setting | Default | Purpose |
| --- | --- | --- |
| `saule.server.path` | `""` | Absolute path to `saule-lsp`. Empty = auto-detect. |
| `saule.server.extraArgs` | `[]` | Extra CLI args for the server. |
| `saule.trace.server` | `"off"` | LSP message tracing (`off` / `messages` / `verbose`). |

## Commands

- **Saule: Restart Language Server** — relaunch the server (e.g. after a
  fresh `cargo build`).
- **Saule: Show Language Server Output** — open the server's output
  channel for logs and traces.

## Syntax-highlighting scopes

Colours come from the user's active theme via these scopes:
`keyword.control`, `keyword.declaration`, `entity.name.type`,
`entity.name.function`, `string.quoted.double`, `comment.line`,
`constant.numeric`, `constant.language`, `variable.language`.
