#!/usr/bin/env bash
# Install the example `engine` native package into WSL/Linux's ~/.saule.
#
# Cargo names the Linux cdylib `libsaule_engine_lib.so`, but the manifest's
# `binary` list uses the un-prefixed `saule_engine_lib.so`, so we rename on
# copy. The manifest is written straight to its install location by the
# `gen-manifest` binary, which renders it from the crate's `#[saule_export]`
# declarations and is built alongside the library.
#
# Build first:  cargo build --release -p saule-engine-lib
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

TARGET_DIR="target"
SO_SRC="$TARGET_DIR/release/libsaule_engine_lib.so"
GEN_MANIFEST="$TARGET_DIR/release/gen-manifest"
SAULE_HOME="${SAULE_HOME:-$HOME/.saule}"

for f in "$SO_SRC" "$GEN_MANIFEST"; do
    if [ ! -f "$f" ]; then
        echo "error: $f not found — run 'cargo build --release -p saule-engine-lib' first" >&2
        exit 1
    fi
done

mkdir -p "$SAULE_HOME/native_packages" "$SAULE_HOME/native_manifests"
cp "$SO_SRC" "$SAULE_HOME/native_packages/saule_engine_lib.so"
"$GEN_MANIFEST" "$SAULE_HOME/native_manifests/engine.toml"

echo "installed:"
echo "  $SAULE_HOME/native_packages/saule_engine_lib.so"
echo "  $SAULE_HOME/native_manifests/engine.toml"
