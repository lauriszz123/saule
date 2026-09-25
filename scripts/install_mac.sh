#!/usr/bin/env bash
# Install the example `engine` native package into macOS' ~/.saule.
#
# The package is one file: the library carries its own description (every
# class, signature and doc comment is compiled into it by `saule-sdk`), so
# installing is copying it into `native_packages/`. Any name works; this keeps
# Cargo's.
#
# Build first:  cargo build --release -p saule-engine-lib
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

LIB="target/release/libsaule_engine_lib.dylib"
SAULE_HOME="${SAULE_HOME:-$HOME/.saule}"

if [ ! -f "$LIB" ]; then
    echo "error: $LIB not found — run 'cargo build --release -p saule-engine-lib' first" >&2
    exit 1
fi

# Apple's linker sometimes leaves a release build's symbol string table
# 4-byte aligned, and macOS 27's dyld refuses to load it ("mis-aligned
# LINKEDIT string pool"). Whether a given link comes out loadable depends on
# a symbol count that changes with any edit, so the library is repaired here
# rather than left to chance. A no-op when the link came out aligned.
if ! python3 "$(dirname "$0")/align_macho_strtab.py" "$LIB"; then
    echo "error: could not make $LIB loadable — see the message above" >&2
    exit 1
fi

mkdir -p "$SAULE_HOME/native_packages"
# An install from before packages carried their own description left a
# second copy under another name, and a separate manifest. Neither is used
# any more, and the old copy would claim the package a second time.
rm -f "$SAULE_HOME/native_packages/saule_engine_lib.dylib" \
      "$SAULE_HOME/native_manifests/engine.toml"
rmdir "$SAULE_HOME/native_manifests" 2>/dev/null || true
cp "$LIB" "$SAULE_HOME/native_packages/"

echo "installed: $SAULE_HOME/native_packages/$(basename "$LIB")"
