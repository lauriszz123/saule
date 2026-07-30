#!/usr/bin/env bash
# Print the version the next release should carry: `<year>.<build+1>`.
#
# The year comes from the major component of `version` in the workspace
# Cargo.toml — the single place the year is written down. The build number is
# one past the highest `v<year>.<n>` tag that already exists, so releases
# number themselves and nobody has to remember what the last one was.
#
# Build numbers restart at 1 each year. That is safe because the year leads,
# so `27.1` still sorts above `26.412`.
#
#   scripts/next-version.sh          # e.g. 26.8
#   scripts/next-version.sh --tag    # e.g. v26.8
#   scripts/next-version.sh --year   # e.g. 26
#
# Reads only; never creates a tag. The release workflow does that.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

year="$(sed -n 's/^version = "\([0-9][0-9]*\)\..*/\1/p' "$ROOT/Cargo.toml" | head -1)"
if [ -z "$year" ]; then
    echo "error: could not read the year from $ROOT/Cargo.toml's [workspace.package] version" >&2
    exit 1
fi

case "${1:-}" in
--year)
    echo "$year"
    exit 0
    ;;
esac

# A shallow clone has no tags, which would silently restart numbering at 1 and
# collide with a published release. Fetch them, and fail loudly rather than
# guess if that is impossible.
if ! git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    echo "error: not a git repository — the build number is derived from tags" >&2
    exit 1
fi
git -C "$ROOT" fetch --tags --quiet 2>/dev/null || true

# Only `v<year>.<digits>` counts. An unrelated tag must never be read as a
# version, so anything else is skipped rather than coerced.
highest="$(
    git -C "$ROOT" tag --list "v${year}.*" |
        sed -n "s/^v${year}\.\([0-9][0-9]*\)$/\1/p" |
        sort -n | tail -1
)"

next=$((${highest:-0} + 1))

case "${1:-}" in
--tag) echo "v${year}.${next}" ;;
"") echo "${year}.${next}" ;;
*)
    echo "usage: $(basename "$0") [--tag|--year]" >&2
    exit 2
    ;;
esac
