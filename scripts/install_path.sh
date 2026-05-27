#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
LINK_DIR="$HOME/.local/bin"
LINK="$LINK_DIR/saule"
ZSH_RC="$HOME/.zshrc"
ZPROFILE="$HOME/.zprofile"
BASH_RC="$HOME/.bashrc"
BIN="$ROOT_DIR/target/release/saule"

if [[ ! -x "$BIN" ]]; then
    echo "no release Saule binary found; building target/release/saule"
    (
        cd "$ROOT_DIR"
        cargo build --release -p saule-cli --bin saule
    )
fi

mkdir -p "$LINK_DIR"
ln -sf "$BIN" "$LINK"
echo "linked: $LINK -> $BIN"

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

ls -l "$LINK"
echo
echo "test with:"
echo "  ~/.local/bin/saule --version"
echo "  zsh -lc 'saule --version'"
