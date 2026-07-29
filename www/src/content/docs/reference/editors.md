---
title: Editor Support
description: Setting up Saule in VS Code, Neovim, and IntelliJ — syntax highlighting plus the saule-lsp language server.
---

Every editor plugin is a thin client. The intelligence lives in **`saule-lsp`**,
a Language Server Protocol implementation that ships with the toolchain, so all
three editors get the same feature set:

- **Diagnostics** — lex, parse, semantic, and type errors, live on every edit
- **Hover** — types and signatures for locals, functions, methods, classes,
  enums, and stdlib members, with generic substitution
- **Go-to-definition** and **find-all-references**
- **Document highlights** and **document symbols** (outline / breadcrumbs)
- **Inlay hints** — inferred local types and parameter-name labels
- **Signature help** — parameter popups while typing call arguments
- **Formatting** — full-document and range formatting

## Build the server first

All three plugins need the binary. From the repo root:

```sh
cargo build --release -p saule-lsp
```

Plugins auto-discover it at `<workspace>/target/release/saule-lsp`, then
`target/debug`, then `saule-lsp` on your `PATH`. Working inside the Saule
repository needs no `PATH` setup at all; working anywhere else does — see
[Installation](/saule/guides/installation/).

Syntax highlighting and indentation are client-side, so they work even without
the server. Everything in the list above does not.

## VS Code

```sh
cd editors/vscode
npm install
npm run compile
```

Then either press <kbd>F5</kbd> (**Run Extension**) from Run & Debug, or package
and install it properly:

```sh
npm install -g @vscode/vsce
vsce package
code --install-extension saule-2026.1.0.vsix
```

### Settings

| Setting | Default | Purpose |
|---|---|---|
| `saule.server.path` | `""` | Absolute path to `saule-lsp`. Empty means auto-detect. |
| `saule.server.extraArgs` | `[]` | Extra CLI arguments for the server. |
| `saule.trace.server` | `"off"` | LSP message tracing — `off`, `messages`, or `verbose`. |

### Commands

- **Saule: Restart Language Server** — relaunch the server, e.g. after a fresh
  `cargo build`.
- **Saule: Show Language Server Output** — open the server's output channel.

## Neovim

The plugin is consumable straight from the repo — nothing needs copying into
`~/.config/nvim/`. Add `editors/nvim` to your runtimepath with whichever plugin
manager you use. With lazy.nvim:

```lua
return {
  {
    dir = "/path/to/saule/editors/nvim",
    name = "saule.vim",
    ft = "saule",
  },
}
```

Then register the language server:

```lua
require("saule.lsp")
```

The Lua helper locates `target/release/saule-lsp` by introspecting its own file
path, so you build once and never touch `$PATH`.

## IntelliJ IDEA

Works in **Community and Ultimate** (and other JetBrains IDEs) — it rides on the
open-source [LSP4IJ](https://github.com/redhat-developer/lsp4ij) client rather
than the Ultimate-only native LSP API.

Build and install:

```sh
cd editors/intellij
./gradlew buildPlugin
```

Then **Settings ▸ Plugins ▸ ⚙ ▸ Install Plugin from Disk…** and pick the zip
from `build/distributions/`.

Beyond the shared LSP features it adds:

- **File ▸ New ▸ Project… ▸ Saule** — scaffolds exactly what `saule init`
  produces.
- **Run configurations** — the gutter icon on any `.sau` file runs the project
  if the file sits inside a `saule.config` tree, and the single file otherwise.

Colouring, brace matching, and indent-while-typing come from a native IntelliJ
lexer rather than the server, so they stay responsive.

:::caution[macOS: IDE launched from the Dock]
GUI applications started from the Dock or Finder do not read `~/.zshrc`, so the
IDE will not see `~/.local/bin` on `PATH`. Set the server path explicitly under
**Settings ▸ Languages & Frameworks ▸ Saule**, or override it with `SAULE_PATH`.
:::
