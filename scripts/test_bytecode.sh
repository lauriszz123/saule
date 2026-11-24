#!/usr/bin/env bash
# Every `tests/run` fixture, built with `saule build` and run as an executable,
# must print exactly what `saule run` prints and exit the same way.
#
# This is the end-to-end check for the bytecode format (`saule-vm`'s
# `bytecode` module) and for `saule build`'s runner: the program is compiled
# in one process, written out, and read back in another that never saw the
# source — so a constant that can't be found again, a field the writer
# forgot, or a runner that sets something up differently shows up as a
# difference here.
#
# Only stdout and the exit code are compared. stderr differs by design: a
# built program has no source text, so error snippets are blank.
#
#   scripts/test_bytecode.sh              every fixture
#   scripts/test_bytecode.sh <area>       one area (`classes`, `stdlib`, …)
#
# `SAULE_BIN` overrides the binary, as for run_tests.sh.
set -u

cd "$(dirname "$0")/.."

SAULE_BIN="${SAULE_BIN:-./target/debug/saule}"
if [ ! -x "$SAULE_BIN" ]; then
  echo "error: $SAULE_BIN not found — run 'cargo build -p saule-cli' first" >&2
  exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

area="${1:-*}"
total=0
failures=0

for f in tests/run/$area/*.sau; do
  total=$((total + 1))
  exe="$work/app"

  expected=$("$SAULE_BIN" run "$f" 2>/dev/null </dev/null)
  expected_code=$?

  if ! build=$("$SAULE_BIN" build "$f" -o "$exe" 2>&1); then
    printf 'FAIL %s (build)\n' "$f"
    echo "$build" | head -8 | sed 's/^/     /'
    failures=$((failures + 1))
    continue
  fi

  actual=$("$exe" 2>"$work/stderr" </dev/null)
  actual_code=$?

  if [ "$actual" != "$expected" ] || [ "$actual_code" != "$expected_code" ]; then
    printf 'FAIL %s (exit %s, expected %s)\n' "$f" "$actual_code" "$expected_code"
    diff <(echo "$expected") <(echo "$actual") | head -6 | sed 's/^/     /'
    head -5 "$work/stderr" | sed 's/^/     /'
    failures=$((failures + 1))
    continue
  fi
  printf 'OK   %s\n' "$f"
done

echo
echo "$((total - failures))/$total fixtures behave the same built as run"
[ "$failures" -eq 0 ]
