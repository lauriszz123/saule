#!/usr/bin/env bash
# Run every `.sau` fixture through the debug build.
#
#   tests/*.sau      must run and exit 0
#   tests/ui/*.sau   must fail — each one exists to pin a specific diagnostic
#
# Exits non-zero if any fixture is on the wrong side of that line, so CI can
# gate on it. `SAULE_BIN` overrides the binary (e.g. to test a release build).
SAULE_BIN="${SAULE_BIN:-./target/debug/saule}"

if [ ! -x "$SAULE_BIN" ]; then
  echo "error: $SAULE_BIN not found — run 'cargo build -p saule-cli' first" >&2
  exit 1
fi

failures=0
total=0

echo '== positive tests =='
for f in tests/*.sau; do
  total=$((total + 1))
  if out=$("$SAULE_BIN" run "$f" 2>&1); then
    printf 'OK   %s\n' "$f"
  else
    printf 'FAIL %s\n' "$f"
    echo "$out" | head -5 | sed 's/^/     /'
    failures=$((failures + 1))
  fi
done

echo
echo '== ui tests (expected to error) =='
for f in tests/ui/*.sau; do
  total=$((total + 1))
  if "$SAULE_BIN" run "$f" >/dev/null 2>&1; then
    printf 'FAIL %s (did not error)\n' "$f"
    failures=$((failures + 1))
  else
    printf 'OK   %s\n' "$f"
  fi
done

echo
if [ "$failures" -eq 0 ]; then
  echo "all $total fixtures behaved as expected"
  exit 0
fi
echo "$failures of $total fixtures failed" >&2
exit 1
