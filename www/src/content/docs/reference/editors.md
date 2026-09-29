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

Each plugin lives in its own repository:

| Editor | Repository |
|---|---|
| VS Code | [lauriszz123/saule-vscode](https://github.com/lauriszz123/saule-vscode) |
| Neovim | [lauriszz123/saule-nvim](https://github.com/lauriszz123/saule-nvim) |
| IntelliJ IDEA | [lauriszz123/saule-intellij](https://github.com/lauriszz123/saule-intellij) |

## Install the server first

All three plugins need the binary, and the
[installer](/saule/guides/installation/) puts it on your `PATH` along with the
`saule` CLI:

```sh
curl -fsSL https://lauriszz123.github.io/saule/install.sh | sh
```

Plugins auto-discover it at `<workspace>/target/release/saule-lsp`, then
`target/debug`, then `saule-lsp` on your `PATH`. Working inside a built
checkout of the language repository therefore needs no `PATH` setup at all;
working anywhere else is what the installer is for.

Syntax highlighting and indentation are client-side, so they work even without
the server. Everything in the list above does not.

## VS Code

```sh
git clone https://github.com/lauriszz123/saule-vscode.git
cd saule-vscode
npm install
npm run compile
```

Then either press <kbd>F5</kbd> (**Run Extension**) from Run & Debug, or package
and install it properly:

```sh
npm install -g @vscode/vsce
vsce package
code --install-extension saule-<version>.vsix
```

### Settings

| Setting | Default | Purpose |
|---|---|---|
| `saule.server.path` | `""` | Absolute path to `saule-lsp`. Empty means auto-detect. |
| `saule.cli.path` | `""` | Absolute path to `saule`, used by the run commands. Empty means auto-detect. |
| `saule.toolchainDir` | `""` | Directory holding both binaries, used when no explicit path is set. |
| `saule.server.extraArgs` | `[]` | Extra CLI arguments for the server. |
| `saule.trace.server` | `"off"` | LSP message tracing — `off`, `messages`, or `verbose`. |

### Commands

- **Saule: Run File** / **Saule: Run Project** — `saule run` on the active file
  or from the workspace root.
- **Saule: Restart Language Server** — relaunch the server, e.g. after a fresh
  `cargo build`.
- **Saule: Show Language Server Output** — open the server's output channel.

## Neovim

The repository is a plugin root, so any plugin manager installs it directly.
With lazy.nvim:

```lua
return {
  { "lauriszz123/saule-nvim", ft = "saule" },
}
```

Then enable the language server. On Neovim 0.11+ the bundled
`lsp/saule.lua` definition is picked up from the runtimepath:

```lua
vim.lsp.enable("saule")
```

With nvim-lspconfig (including NvChad), register it through the helper instead,
which picks up your shared `on_attach` and `capabilities`:

```lua
require("saule.lsp").setup()
```

Either way the server is located by walking up from the file you are editing,
looking for `target/release/saule-lsp` and then `target/debug`, before falling
back to `$PATH` — so a built checkout needs no configuration, and everything
else is covered by the installer. `vim.g.saule_lsp_path` overrides it.

## IntelliJ IDEA

Works in **Community and Ultimate** (and other JetBrains IDEs) — it rides on the
open-source [LSP4IJ](https://github.com/redhat-developer/lsp4ij) client rather
than the Ultimate-only native LSP API.

Build and install:

```sh
git clone https://github.com/lauriszz123/saule-intellij.git
cd saule-intellij
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
IDE will not see `~/.saule/bin` on `PATH`. Set the toolchain directory
explicitly under **Settings ▸ Languages & Frameworks ▸ Saule**, or override it
with `SAULE_PATH`.
:::

## Syntax highlighting

The TextMate grammar is written once, in the language repository at
`grammar/saule.tmLanguage.json`, next to the lexer it has to agree with. This
site reads it at build time, and the VS Code extension ships a copy that
`npm run sync:grammar` refreshes — so a keyword added in one place shows up
everywhere.
