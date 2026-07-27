#!/usr/bin/env bash
# Link the Saule toolchain into ~/.local/bin and put that on PATH.
#
# Both binaries are installed. `saule-lsp` matters as much as `saule`: the
# editor plugins look for the language server in the project's `target/`
# directory first and fall back to PATH, so without it on PATH the LSP only
# works inside this repo — in any other project you get syntax highlighting
# and indentation (both client-side) but no formatting, hover or diagnostics.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
LINK_DIR="$HOME/.local/bin"
ZSH_RC="$HOME/.zshrc"
ZPROFILE="$HOME/.zprofile"
BASH_RC="$HOME/.bashrc"
RELEASE_DIR="$ROOT_DIR/target/release"

BINS=(saule saule-lsp)

needs_build=false
for bin in "${BINS[@]}"; do
    if [[ ! -x "$RELEASE_DIR/$bin" ]]; then
        needs_build=true
    fi
done

if [[ "$needs_build" == true ]]; then
    echo "missing release binaries; building saule and saule-lsp"
    (
        cd "$ROOT_DIR"
        cargo build --release -p saule-cli -p saule-lsp
    )
fi

mkdir -p "$LINK_DIR"
for bin in "${BINS[@]}"; do
    src="$RELEASE_DIR/$bin"
    if [[ ! -x "$src" ]]; then
        echo "error: $src not found after build" >&2
        exit 1
    fi
    ln -sf "$src" "$LINK_DIR/$bin"
    echo "linked: $LINK_DIR/$bin -> $src"
done

ensure_path_line() {
    local rc_file="$1"
    if [[ ! -f "$rc_file" ]]; then
        touch "$rc_file"
    fi

    if ! grep -Fqs 'export PATH="$HOME/.local/bin:$PATH"' "$rc_file"; then
        echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$rc_file"
        echo "added PATH to ${rc_file/#$HOME/~}"
    else
        echo "PATH entry already present in ${rc_file/#$HOME/~}"
    fi
}

ensure_path_line "$ZSH_RC"
ensure_path_line "$ZPROFILE"
ensure_path_line "$BASH_RC"

echo
echo "test with:"
echo "  ~/.local/bin/saule --version"
echo "  zsh -lc 'saule --version'"
echo "  zsh -lc 'saule-lsp --version'"
echo
echo "note: GUI apps launched from the Dock/Finder on macOS do NOT read"
echo "      ~/.zshrc or ~/.zprofile, so IDEs started that way will not see"
echo "      $LINK_DIR on PATH. For IntelliJ/Android Studio, set the server"
echo "      path explicitly in Settings > Languages & Frameworks > Saule:"
echo "        $RELEASE_DIR/saule-lsp"
