#!/usr/bin/env bash
# Install the example `engine` native package into WSL/Linux's ~/.saule.
#
# Cargo names the Linux cdylib `libsaule_engine_lib.so`, but the manifest's
# `binary` list uses the un-prefixed `saule_engine_lib.so`, so we rename on
# copy. Run scripts/build_wsl.sh -p saule-engine-lib first.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

TARGET_DIR="./target"
SO_SRC="$TARGET_DIR/release/libsaule_engine_lib.dylib"
SAULE_HOME="${SAULE_HOME:-$HOME/.saule}"

if [ ! -f "$SO_SRC" ]; then
    echo "error: $SO_SRC not found — run scripts/install_mac.sh -p saule-engine-lib first" >&2
    exit 1
fi

mkdir -p "$SAULE_HOME/native_packages" "$SAULE_HOME/native_manifests"
cp "$SO_SRC" "$SAULE_HOME/native_packages/saule_engine_lib.dylib"
cp "$TARGET_DIR/release/engine.toml" "$SAULE_HOME/native_manifests/engine.toml"

echo "installed:"
echo "  $SAULE_HOME/native_packages/saule_engine_lib.dylib"
echo "  $SAULE_HOME/native_manifests/engine.toml"
