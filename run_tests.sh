#!/usr/bin/env bash
# Run every `.sau` fixture through the debug build.
#
#   tests/*.sau      must run and exit 0
#   tests/ui/*.sau   must fail — each one exists to pin a specific diagnostic
#
# Exits non-zero if any fixture is on the wrong side of that line, so CI can
# gate on it. `SAULE_BIN` overrides the binary (e.g. to test a release build).
#
# SAULE_DIFF=1 runs every fixture under **both** engines and requires the
# output to be identical, not just the exit status.
#
#   Exit status alone is a weak check, and it is weak in exactly the place
#   that matters: a VM bug that prints the wrong value while still exiting 0
#   passes silently. `OpToString` did precisely that — `<instance of Money>`
#   where the tree-walker prints `300c`, no error anywhere. This mode turns
#   all 235 fixtures into conformance tests at no authoring cost, which is
#   what `VM_TASKS.md` asks for under "Cross-cutting: testing".
SAULE_BIN="${SAULE_BIN:-./target/debug/saule}"
SAULE_DIFF="${SAULE_DIFF:-}"

if [ ! -x "$SAULE_BIN" ]; then
  echo "error: $SAULE_BIN not found — run 'cargo build -p saule-cli' first" >&2
  exit 1
fi

failures=0
total=0

# A fallback note is the VM *declining* to compile something, which is a
# designed outcome and not a behavioural difference. Everything else must
# match character for character.
strip_notes() {
  grep -v '^note: the bytecode compiler does not handle ' || true
}

# Fixtures whose engines are *meant* to disagree. Each needs a reason, and
# the count is reported at the end so this list cannot grow unnoticed —
# an exemption you cannot see is just a failing test you stopped reading.
diff_exempt() {
  case "$1" in
    # VM_DESIGN.md §6.4: the VM counts *frames* and deliberately allows two
    # orders of magnitude more nesting than the tree-walker's eval depth, so
    # the limit named in the message differs by design. Both still report a
    # stack overflow, which is the behaviour this fixture pins.
    tests/ui/stack_overflow_recursion.sau) return 0 ;;
    # Same rule, reached through the re-entrancy path: a comparator that
    # sorts with itself. Each level crosses the engine boundary, so the VM
    # bounds it with the interpreter's shared depth guard rather than with
    # `max_frames` — and the limit each engine names still differs by design.
    tests/ui/stack_overflow_reentrant.sau) return 0 ;;
    *) return 1 ;;
  esac
}
exempted=0

# Run one fixture under both engines and compare. Echoes a diff and returns
# non-zero when they disagree.
compare_engines() {
  local f="$1" interp vm
  interp=$(SAULE_ENGINE=interp "$SAULE_BIN" run "$f" 2>&1 | strip_notes)
  vm=$(SAULE_ENGINE=vm "$SAULE_BIN" run "$f" 2>&1 | strip_notes)
  if [ "$interp" = "$vm" ]; then
    return 0
  fi
  diff <(printf '%s\n' "$interp") <(printf '%s\n' "$vm") | head -12 | sed 's/^/     /'
  return 1
}

echo '== positive tests =='
for f in tests/*.sau; do
  total=$((total + 1))
  if ! out=$("$SAULE_BIN" run "$f" 2>&1); then
    printf 'FAIL %s\n' "$f"
    echo "$out" | head -5 | sed 's/^/     /'
    failures=$((failures + 1))
    continue
  fi
  if [ -n "$SAULE_DIFF" ] && ! diff_exempt "$f"; then
    if ! d=$(compare_engines "$f"); then
      printf 'FAIL %s (engines disagree)\n' "$f"
      printf '%s\n' "$d"
      failures=$((failures + 1))
      continue
    fi
  fi
  printf 'OK   %s\n' "$f"
done

echo
echo '== ui tests (expected to error) =='
for f in tests/ui/*.sau; do
  total=$((total + 1))
  if "$SAULE_BIN" run "$f" >/dev/null 2>&1; then
    printf 'FAIL %s (did not error)\n' "$f"
    failures=$((failures + 1))
    continue
  fi
  # These exist to pin a specific *diagnostic*, so under SAULE_DIFF the
  # message itself has to match too — a VM that errors for a different
  # reason is not the same behaviour.
  if [ -n "$SAULE_DIFF" ] && ! diff_exempt "$f"; then
    if ! d=$(compare_engines "$f"); then
      printf 'FAIL %s (engines disagree)\n' "$f"
      printf '%s\n' "$d"
      failures=$((failures + 1))
      continue
    fi
  fi
  printf 'OK   %s\n' "$f"
done

echo
if [ -n "$SAULE_DIFF" ]; then
  for f in tests/*.sau tests/ui/*.sau; do
    diff_exempt "$f" && exempted=$((exempted + 1))
  done
  if [ "$exempted" -gt 0 ]; then
    echo "note: $exempted fixture(s) exempt from the engine diff — see diff_exempt()"
  fi
fi
if [ "$failures" -eq 0 ]; then
  if [ -n "$SAULE_DIFF" ]; then
    echo "all $total fixtures behaved as expected, and both engines agree on their output"
    exit 0
  fi
  echo "all $total fixtures behaved as expected"
  exit 0
fi
echo "$failures of $total fixtures failed" >&2
exit 1
