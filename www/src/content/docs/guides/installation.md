---
title: Installation
description: Build the Saule toolchain from source and put the compiler and language server on your PATH.
sidebar:
  order: 2
---

Saule is written in Rust and currently installs from source. You need a Rust
toolchain on **edition 2024** — Rust 1.85 or newer.

:::tip[Just want to try the language?]
The [playground](/saule/play/) runs Saule in your browser with nothing to
install.
:::

## Build from source

```sh
git clone https://github.com/lauriszz123/saule.git
cd saule
cargo build --release -p saule-cli -p saule-lsp
```

That produces two binaries in `target/release/`:

| Binary | Purpose |
|---|---|
| `saule` | The compiler and runtime — `run`, `fmt`, `init` |
| `saule-lsp` | The language server, used by every editor plugin |

Install **both**. The editor plugins look for the language server in the
project's `target/` directory first and fall back to `PATH`; without
`saule-lsp` installed you get syntax highlighting and indentation (which are
client-side) but no formatting, hover, or diagnostics outside this repository.

## Put it on your PATH

### macOS and Linux

The repository ships a script that symlinks both binaries into
`~/.local/bin` and adds that directory to your shell profile:

```sh
./scripts/install_path.sh
```

It builds the release binaries first if they are missing. Verify with:

```sh
saule --version
```

:::caution[macOS GUI applications]
Applications launched from the Dock or Finder do not read `~/.zshrc` or
`~/.zprofile`, so an IDE started that way will not see `~/.local/bin` on
`PATH`. For IntelliJ, set the language-server path explicitly under
**Settings › Languages & Frameworks › Saule**.
:::

### Windows

Add `target\release` to your `PATH`, or copy `saule.exe` and `saule-lsp.exe`
into a directory that is already on it.

## Editor support

Plugins for VS Code, Neovim, and IntelliJ live in
[`editors/`](https://github.com/lauriszz123/saule/tree/main/editors). See
[Editor Support](/saule/reference/editors/) for per-editor setup.

## Next steps

Write something: **[Your First Program](/saule/guides/first-program/)**.
