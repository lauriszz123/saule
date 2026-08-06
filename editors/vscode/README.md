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
- **Signature help** — parameter popups while typing call arguments, and
  whenever the caret moves back inside a call's parens.
- **Formatting** — full-document and range formatting.

Provided by the extension itself, so they work with or without the server:

- **Indentation while typing** — Enter indents the new line, and block-closing
  keywords dedent as you finish them: `end`, `until`, `else`, `elseif`,
  `catch` and `case` snap to the right level. Saule closes blocks with words
  rather than braces, so there is no `}` for the usual dedent to hook onto.
  Driven by `editor.formatOnType`, which this extension turns on for `.sau`
  files (along with two-space indentation, matching `saule fmt`).
- **Run commands** — run the current file or the whole project in a terminal.

## Build the toolchain (one time)

From the repo root:

```powershell
cargo build --release
```

The extension discovers the binaries in the same order as the IntelliJ plugin,
so both pick the same build in the same project:

1. `SAULE_LSP_PATH` / `SAULE_PATH` environment variables.
2. `saule.server.path` / `saule.cli.path`, or `saule.toolchainDir`.
3. Cargo build output, walking **up** from each workspace folder looking for
   `target/release` then `target/debug`. Walking up is what lets you open a
   sub-folder (say `examples/todo-app`) and still find the workspace-root build
   output — and the directory holding that `target/` becomes the server's
   working directory.
4. `saule-lsp` / `saule` on your `PATH`.

No `PATH` setup is needed when you work inside the Saule repo.

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
| `saule.cli.path` | `""` | Absolute path to `saule`, used by the run commands. Empty = auto-detect. |
| `saule.toolchainDir` | `""` | Directory holding both binaries, used when no explicit path is set. |
| `saule.server.extraArgs` | `[]` | Extra CLI args for the server. |
| `saule.trace.server` | `"off"` | LSP message tracing (`off` / `messages` / `verbose`). |

The extension also sets `editor.tabSize: 2`, `editor.insertSpaces: true` and
`editor.formatOnType: true` for `[saule]` files. These are defaults — your own
settings still win.

## Commands

- **Saule: Run File** — `saule run <file>` on the active buffer, saving it
  first. Also on the editor context menu.
- **Saule: Run Project** — `saule run` from the workspace root.
- **Saule: Restart Language Server** — relaunch the server (e.g. after a
  fresh `cargo build`).
- **Saule: Show Language Server Output** — open the server's output
  channel for logs and traces.

## Editor parity

`src/indent.ts` is a port of the IntelliJ plugin's `SauleIndentModel` and
shares its test corpus with the Neovim integration's `lua/saule/indent.lua`.
All three are derived from the printer in `crates/saule-fmt/src/lib.rs`. If you
change one, change all of them and re-run every suite:

```powershell
npm test
```

## Syntax-highlighting scopes

Colours come from the user's active theme via these scopes:
`keyword.control`, `keyword.declaration`, `entity.name.type`,
`entity.name.function`, `string.quoted.double`, `comment.line`,
`constant.numeric`, `constant.language`, `variable.language`.
