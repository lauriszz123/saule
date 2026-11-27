#!/usr/bin/env bash
# Every fixture and every benchmark, run with numeric loops on plain numbers
# (`saule-vm`'s `vm/plain.rs`) and with every loop left to the main loop
# (`SAULE_PLAIN=0`), must print the same, fail the same and exit the same.
#
# A plan runs a loop's instructions on numbers taken out of their
# registers, and gives an instruction back to the main loop whenever that
# would do anything but the ordinary. This is the check that it gives back
# everything it should: standard output, standard error (an error names
# its line and shows it) and the exit code are all compared.
#
#   scripts/test_plain.sh              every fixture and benchmark
#   scripts/test_plain.sh <area>       one area of `tests/run` and `tests/errors`
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
files=(tests/run/$area/*.sau tests/errors/$area/*.sau)
if [ "$area" = "*" ]; then
  files+=(benchmarks/sau/*.sau)
fi

total=0
failures=0
planned=0

for f in "${files[@]}"; do
  [ -f "$f" ] || continue
  total=$((total + 1))

  "$SAULE_BIN" run "$f" >"$work/on.out" 2>"$work/on.err" </dev/null
  on=$?
  SAULE_PLAIN=0 "$SAULE_BIN" run "$f" >"$work/off.out" 2>"$work/off.err" </dev/null
  off=$?

  if [ "$on" != "$off" ] || ! cmp -s "$work/on.out" "$work/off.out" ||
    ! cmp -s "$work/on.err" "$work/off.err"; then
    printf 'FAIL %s (exit %s with plans, %s without)\n' "$f" "$on" "$off"
    diff "$work/off.out" "$work/on.out" | head -6 | sed 's/^/     /'
    diff "$work/off.err" "$work/on.err" | head -6 | sed 's/^/     /'
    failures=$((failures + 1))
    continue
  fi
  # How many of them had a loop to run this way at all.
  if SAULE_PLAIN=trace "$SAULE_BIN" run "$f" 2>&1 >/dev/null </dev/null | grep -q '^plain: '; then
    planned=$((planned + 1))
  fi
done

echo "$((total - failures))/$total programs behave the same with and without plans ($planned of them have a loop with one)"
[ "$failures" -eq 0 ]
