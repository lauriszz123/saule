#!/bin/sh
# Build one release archive for $TRIPLE, on Linux or macOS.
#
# The body shared by the Linux and macOS build jobs. It lives in a file rather
# than in .circleci/config.yml because a CircleCI job cannot include another
# job's steps, and two copies of a version check are two chances to skip one.
# Windows is PowerShell and stays inline in the config.
#
# Inputs, all from the job:
#   TRIPLE      the target triple to build
#   EMULATOR    optional; how to execute a foreign binary for the version
#               check (`qemu-aarch64`, `arch -x86_64`)
#   meta/VERSION  written by the `version` job and read from the workspace —
#               the single decision every archive in a release agrees on
set -eu

: "${TRIPLE:?TRIPLE is not set}"

if [ ! -f meta/VERSION ]; then
    echo "error: meta/VERSION is missing — the workspace from the \`version\` job was not attached" >&2
    exit 1
fi

# The one input that decides what the binaries report. Everything else —
# `saule --version`, `Saule.version`, the LSP handshake — reads it back out of
# the binary.
SAULE_VERSION="$(cat meta/VERSION)"
export SAULE_VERSION
echo "Building Saule $SAULE_VERSION for $TRIPLE"

cargo build --release --locked --target "$TRIPLE" -p saule-cli -p saule-lsp

# Every archive is executed here rather than shipped unverified: natively where
# the runner matches, under qemu-user for the aarch64 Linux cross build, and
# under Rosetta for the x86_64 Apple build. A dev-marked or misnumbered binary
# in a release archive is the one failure that would be invisible until a user
# reported it.
bin="target/$TRIPLE/release/saule"
if got="$(${EMULATOR:-} "$bin" --version 2>&1)"; then
    echo "$bin --version -> $got"
    if [ "$got" != "saule $SAULE_VERSION" ]; then
        echo "error: expected \`saule $SAULE_VERSION\`, got \`$got\`" >&2
        exit 1
    fi
elif [ -n "${EMULATOR:-}" ]; then
    # The binary is for another architecture and this machine cannot run it.
    # The usual cause is a CI image without Rosetta. Shipping it unverified is
    # the lesser evil — the alternative is that no macOS x86_64 archive exists
    # at all — but it is said loudly, because it is the one archive in the
    # release that nothing has executed.
    echo "warning: could not execute $bin under \`${EMULATOR}\`, so its version" >&2
    echo "         was NOT verified. Output was: $got" >&2
    echo "         Install Rosetta on the builder, or verify this archive by hand." >&2
else
    echo "error: $bin could not be executed on its native platform: $got" >&2
    exit 1
fi

name="saule-$SAULE_VERSION-$TRIPLE"
staging="dist/$name"
mkdir -p "$staging"
cp "target/$TRIPLE/release/saule" "target/$TRIPLE/release/saule-lsp" "$staging/"
cp README.md "$staging/"
# Written as an `if` rather than `[ -f LICENSE ] && cp …`: under `set -e` a
# failing `&&` chain in tail position aborts the script, so the terse spelling
# would break every build until LICENSE exists.
if [ -f LICENSE ]; then
    cp LICENSE "$staging/"
else
    echo "warning: no LICENSE file. Archives are not redistributable without one." >&2
fi

tar czf "dist/$name.tar.gz" -C dist "$name"
rm -rf "$staging"
ls -la dist
