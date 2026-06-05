#!/usr/bin/env bash
# Install the example `engine` native package into WSL/Linux's ~/.saule.
#
# Cargo names the Linux cdylib `libsaule_engine_lib.so`, but the manifest's
# `binary` list uses the un-prefixed `saule_engine_lib.so`, so we rename on
# copy. The manifest (`engine.toml`) is generated from the crate's
# `#[saule_export]` declarations by the `gen-manifest` binary, which is built
# alongside the library. Build saule-engine-lib (release) first.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/saule-target-wsl}"
SO_SRC="$TARGET_DIR/release/libsaule_engine_lib.so"
GEN_BIN="$TARGET_DIR/release/gen-manifest"
MANIFEST_SRC="$TARGET_DIR/release/engine.toml"
SAULE_HOME="${SAULE_HOME:-$HOME/.saule}"

if [ ! -f "$SO_SRC" ]; then
    echo "error: $SO_SRC not found — build saule-engine-lib (release) first" >&2
    exit 1
fi

if [ ! -x "$GEN_BIN" ]; then
    echo "error: $GEN_BIN not found — build saule-engine-lib (release) first" >&2
    exit 1
fi

# Regenerate the manifest from the #[saule_export] declarations.
"$GEN_BIN" "$MANIFEST_SRC"

mkdir -p "$SAULE_HOME/native_packages" "$SAULE_HOME/native_manifests"
cp "$SO_SRC" "$SAULE_HOME/native_packages/saule_engine_lib.so"
cp "$MANIFEST_SRC" "$SAULE_HOME/native_manifests/engine.toml"

echo "installed:"
echo "  $SAULE_HOME/native_packages/saule_engine_lib.so"
echo "  $SAULE_HOME/native_manifests/engine.toml"
