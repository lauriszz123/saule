#!/usr/bin/env bash
# Build the Saule interpreter to WebAssembly for the playground.
#
# Output lands in www/src/lib/saule_wasm/, which `src/lib/runtime.ts` imports
# dynamically so the module is only fetched when someone opens /play/.
#
# Usage:
#   www/scripts/build-wasm.sh           # release build
#   www/scripts/build-wasm.sh --debug   # faster to compile, much larger
set -euo pipefail

PROFILE="release"
if [[ "${1:-}" == "--debug" ]]; then
	PROFILE="debug"
fi

WWW_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REPO_ROOT="$(cd "$WWW_DIR/.." && pwd)"
OUT_DIR="$WWW_DIR/src/lib/saule_wasm"
TARGET="wasm32-unknown-unknown"

# A machine can have both a Homebrew rust and a rustup one, and
# `rustup target add` installs the wasm std into rustup's. Cargo takes `rustc`
# from PATH, so if the Homebrew one wins you get a misleading
# "can't find crate for `core`" that looks like a missing target.
#
# Ask rustup where its binaries are and pin both. `rustup run` is *not*
# enough: it fixes the command it launches, but cargo then spawns `rustc` by
# name and picks up whatever PATH offers first, which lands back on Homebrew's.
# Setting RUSTC explicitly is what actually works. Don't derive the toolchain
# directory by hand either — the host triple ("aarch64") is not what
# `uname -m` prints ("arm64").
if command -v rustup >/dev/null 2>&1; then
	if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
		echo "error: the $TARGET target is not installed." >&2
		echo "       Add it with:  rustup target add $TARGET" >&2
		exit 1
	fi
	CARGO=("$(rustup which cargo)")
	RUSTC="$(rustup which rustc)"
	export RUSTC
else
	CARGO=(cargo)
fi

# Locate wasm-bindgen. `cargo install` puts it in cargo's bin directory, which
# is only on PATH if rustup added it — someone using a Homebrew rust will have
# the binary installed and still not be able to run it by name. Look there
# directly rather than making that the user's problem.
WASM_BINDGEN=""
if command -v wasm-bindgen >/dev/null 2>&1; then
	WASM_BINDGEN="$(command -v wasm-bindgen)"
else
	for candidate in "${CARGO_HOME:-$HOME/.cargo}/bin/wasm-bindgen" "$HOME/.cargo/bin/wasm-bindgen"; do
		if [[ -x "$candidate" ]]; then
			WASM_BINDGEN="$candidate"
			break
		fi
	done
fi

if [[ -z "$WASM_BINDGEN" ]]; then
	# The CLI's version must match the `wasm-bindgen` crate the module was
	# compiled against — a mismatch fails with a schema error, not a warning.
	VERSION="$(grep -A 1 '^name = "wasm-bindgen"$' "$REPO_ROOT/Cargo.lock" | grep '^version' | head -1 | cut -d'"' -f2)"
	echo "error: wasm-bindgen not found." >&2
	echo "       Looked on PATH and in ${CARGO_HOME:-$HOME/.cargo}/bin." >&2
	echo "       Install the matching version with:" >&2
	echo "         cargo install wasm-bindgen-cli --version ${VERSION:-0.2}" >&2
	exit 1
fi

# A CLI older or newer than the crate fails with an opaque schema-version
# error deep in the generated glue. Catch it here, where the fix is obvious.
CRATE_VERSION="$(grep -A 1 '^name = "wasm-bindgen"$' "$REPO_ROOT/Cargo.lock" | grep '^version' | head -1 | cut -d'"' -f2)"
CLI_VERSION="$("$WASM_BINDGEN" --version 2>/dev/null | awk '{print $2}')"
if [[ -n "$CRATE_VERSION" && -n "$CLI_VERSION" && "$CRATE_VERSION" != "$CLI_VERSION" ]]; then
	echo "error: wasm-bindgen version mismatch." >&2
	echo "       CLI:   $CLI_VERSION  ($WASM_BINDGEN)" >&2
	echo "       Crate: $CRATE_VERSION  (from Cargo.lock)" >&2
	echo "       These must match. Reinstall with:" >&2
	echo "         cargo install wasm-bindgen-cli --version $CRATE_VERSION --force" >&2
	exit 1
fi

echo "==> Compiling saule-wasm ($PROFILE)"
BUILD_ARGS=(build -p saule-wasm --target "$TARGET")
if [[ "$PROFILE" == "release" ]]; then
	BUILD_ARGS+=(--release)
fi
(cd "$REPO_ROOT" && "${CARGO[@]}" "${BUILD_ARGS[@]}")

WASM="$REPO_ROOT/target/$TARGET/$PROFILE/saule_wasm.wasm"
if [[ ! -f "$WASM" ]]; then
	echo "error: expected $WASM to exist after the build" >&2
	exit 1
fi

echo "==> Generating JS bindings"
rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"
# `--target web` emits an ES module with an exported `init()`, which is what a
# dynamic `import()` from Vite wants. No bundler-specific glue.
"$WASM_BINDGEN" "$WASM" --out-dir "$OUT_DIR" --target web --no-typescript

# `wasm-opt` is optional: it typically strips another 15-25%, but the build
# must not require it.
if command -v wasm-opt >/dev/null 2>&1; then
	echo "==> Optimising with wasm-opt"
	wasm-opt -Oz "$OUT_DIR/saule_wasm_bg.wasm" -o "$OUT_DIR/saule_wasm_bg.wasm"
else
	echo "    (wasm-opt not found — skipping; install binaryen to shrink the module)"
fi

SIZE=$(du -h "$OUT_DIR/saule_wasm_bg.wasm" | cut -f1)
GZIP=$(gzip -c "$OUT_DIR/saule_wasm_bg.wasm" | wc -c | awk '{printf "%.0fK", $1/1024}')
echo
echo "Built $OUT_DIR/saule_wasm_bg.wasm  ($SIZE raw, ~$GZIP gzipped)"
