---
title: Installation
description: Install the Saule toolchain with one command, or build it from source.
sidebar:
  order: 2
---

:::tip[Just want to try the language?]
The [playground](/saule/play/) runs Saule in your browser with nothing to
install.
:::

## One command

**macOS and Linux**

```sh
curl -fsSL https://lauriszz123.github.io/saule/install.sh | sh
```

**Windows**

```powershell
irm https://lauriszz123.github.io/saule/install.ps1 | iex
```

Then open a new terminal:

```sh
saule --version
```

The installer picks the right build for your machine, verifies it against the
release's `SHA256SUMS`, installs both binaries into `~/.saule/bin`
(`%USERPROFILE%\.saule\bin` on Windows), and puts that directory on your
`PATH`. Running it again upgrades in place.

### Reading it first

Piping a script into a shell is worth being suspicious of. Both scripts are
short and are served from this site over HTTPS; read either before running it:

```sh
curl -fsSL https://lauriszz123.github.io/saule/install.sh | less
```

### Options

Both installers are configured by environment variables.

| Variable | Effect |
|---|---|
| `SAULE_VERSION` | Install a specific version, e.g. `26.7`, instead of the latest |
| `SAULE_HOME` | Install root. Used **verbatim** — it *is* the directory, not a parent to append `.saule` to. Defaults to `~/.saule` |
| `SAULE_NO_MODIFY_PATH=1` | Install the binaries but leave your shell profile and `PATH` alone |

```sh
SAULE_VERSION=26.7 curl -fsSL https://lauriszz123.github.io/saule/install.sh | sh
```

### What gets installed

| Binary | Purpose |
|---|---|
| `saule` | The compiler and runtime — `run`, `fmt`, `init` |
| `saule-lsp` | The language server, used by every editor plugin |

Both, always. The editor plugins look for the language server in the project's
`target/` directory first and fall back to `PATH`; without `saule-lsp`
installed you get syntax highlighting and indentation (which are client-side)
but no formatting, hover, or diagnostics.

## Installing by hand

Every release publishes a signed-checksum archive per platform on the
[releases page](https://gitlab.com/lauriszz123/saule/-/releases).

```sh
# Verify before unpacking — SHA256SUMS covers every archive in the release.
shasum -a 256 -c SHA256SUMS --ignore-missing

tar xzf saule-<version>-<triple>.tar.gz
mkdir -p ~/.saule/bin
cp saule-<version>-<triple>/saule saule-<version>-<triple>/saule-lsp ~/.saule/bin/
export PATH="$HOME/.saule/bin:$PATH"
```

Supported triples:

| Platform | Triple |
|---|---|
| macOS, Apple Silicon | `aarch64-apple-darwin` |
| macOS, Intel | `x86_64-apple-darwin` |
| Linux x86-64, glibc | `x86_64-unknown-linux-gnu` |
| Linux x86-64, musl (Alpine, or an older glibc than the build's) | `x86_64-unknown-linux-musl` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |
| Windows x86-64 | `x86_64-pc-windows-msvc` |

:::caution[macOS quarantine]
If you downloaded the archive in a browser rather than with `curl`, macOS
flags it: `xattr -dr com.apple.quarantine ~/.saule/bin`. The one-line
installer is not affected, because `curl` does not set the flag.
:::

## Building from source

Saule is written in Rust and needs a toolchain on **edition 2024** — Rust 1.85
or newer.

```sh
git clone https://gitlab.com/lauriszz123/saule.git
cd saule
cargo build --release -p saule-cli -p saule-lsp
```

That produces both binaries in `target/release/`. Copy them into
`~/.saule/bin`, or add `target/release` to your `PATH`.

:::caution[macOS GUI applications]
Applications launched from the Dock or Finder do not read `~/.zshrc` or
`~/.zprofile`, so an IDE started that way will not see `~/.saule/bin` on
`PATH`. For IntelliJ, set the language-server path explicitly under
**Settings › Languages & Frameworks › Saule**.
:::

## Editor support

Plugins for VS Code, Neovim, and IntelliJ live in
[`editors/`](https://gitlab.com/lauriszz123/saule/-/tree/main/editors). See
[Editor Support](/saule/reference/editors/) for per-editor setup.

## Next steps

Write something: **[Your First Program](/saule/guides/first-program/)**.
