#!/usr/bin/env bash
cd ~/Codai/rust/saule
echo '== positive tests =='
for f in tests/*.sau; do
  out=$(./target/debug/saule "$f" 2>&1)
  rc=$?
  if [ $rc -eq 0 ]; then
    printf 'OK   %s\n' "$f"
  else
    printf 'FAIL %s\n' "$f"
    echo "$out" | head -5 | sed 's/^/     /'
  fi
done
echo
echo '== ui tests (expected to error) =='
for f in tests/ui/*.sau; do
  out=$(./target/debug/saule "$f" 2>&1)
  rc=$?
  if [ $rc -ne 0 ]; then
    printf 'OK   %s\n' "$f"
  else
    printf 'FAIL %s (did not error)\n' "$f"
  fi
done
