#!/usr/bin/env bash
# Helper: build the workspace inside WSL/Linux with a Linux-only target dir
# so artifacts don't collide with the Windows `target/` tree.
set -euo pipefail
cd "$(dirname "$0")/.."

# Prefer the rustup-managed toolchain over any old distro cargo on PATH
# (Ubuntu's apt cargo is too old for resolver "3" / edition 2024).
if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/saule-target-wsl}"
cargo build "$@"
