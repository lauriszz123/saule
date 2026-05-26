#!/usr/bin/env bash
set -euo pipefail

BIN="$HOME/Codai/rust/saule/target/debug/saule"
LINK_DIR="$HOME/.local/bin"
LINK="$LINK_DIR/saule"

mkdir -p "$LINK_DIR"
ln -sf "$BIN" "$LINK"
echo "linked: $LINK -> $BIN"

if ! grep -qs '.local/bin' "$HOME/.bashrc" 2>/dev/null; then
    echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.bashrc"
    echo "added PATH to ~/.bashrc"
else
    echo "PATH entry already present in ~/.bashrc"
fi

ls -l "$LINK"
