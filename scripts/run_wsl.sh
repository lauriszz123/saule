#!/usr/bin/env bash
# Build saule-cli for Linux and run a .sau file. Usage:
#   scripts/run_wsl.sh examples/native-package/demo.sau
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/saule-target-wsl}"
cargo build --release
exec "$CARGO_TARGET_DIR/release/saule" run "$@"
