#!/usr/bin/env bash
# Run every `.sau` fixture through the debug build.
#
#   tests/*.sau      must run and exit 0
#   tests/ui/*.sau   must fail — each one exists to pin a specific diagnostic
#
# and in both cases print exactly what `expected/<name>.out` beside it says —
# stdout and stderr together, diagnostics included. Exit status alone is a
# weak check, and weak in exactly the place that matters: a bug that prints
# the wrong value while still exiting 0 passes silently.
#
# The expected outputs were recorded from the tree-walking interpreter, the
# engine that defined the language until it was removed; every one was also
# the bytecode VM's output then, so they still say what the language means.
#
# Exits non-zero if any fixture is on the wrong side of either line, so CI
# can gate on it. `SAULE_BIN` overrides the binary (e.g. to test a release
# build).
#
# SAULE_BLESS=1 writes each fixture's output to its expected file instead of
# comparing — for a new fixture, or a deliberate change in behaviour. Read
# the resulting diff before committing it: it is the specification.
SAULE_BIN="${SAULE_BIN:-}"
SAULE_BLESS="${SAULE_BLESS:-}"

# Default to whichever of the two names exists: Windows builds `saule.exe`.
if [ -z "$SAULE_BIN" ]; then
  if [ -x ./target/debug/saule ]; then
    SAULE_BIN=./target/debug/saule
  else
    SAULE_BIN=./target/debug/saule.exe
  fi
fi

if [ ! -x "$SAULE_BIN" ]; then
  echo "error: $SAULE_BIN not found — run 'cargo build -p saule-cli' first" >&2
  exit 1
fi

failures=0
total=0

# Fixtures whose output cannot be pinned, each with a reason. Their exit
# status is still checked. Counted at the end, so this list cannot grow
# unnoticed — an exemption you cannot see is a failing test you stopped
# reading.
output_exempt() {
  case "$1" in
    # Both profiles report, but not the same way: a level of re-entrant
    # nesting costs several times more stack in a debug build, so it runs
    # out of the thread's stack where a release build reaches the level
    # count first. The message differs accordingly — and so does how many
    # times `Table.sort: comparator failed:` is wrapped around it. That the
    # run *reports* rather than dying is what this fixture pins.
    tests/ui/stack_overflow_reentrant.sau) return 0 ;;
    *) return 1 ;;
  esac
}
exempted=0

# Compare (or, when blessing, record) one fixture's output. Echoes a diff
# and returns non-zero when it differs.
check_output() {
  local f="$1" out="$2" expected
  expected="$(dirname "$f")/expected/$(basename "$f" .sau).out"
  if [ -n "$SAULE_BLESS" ]; then
    printf '%s\n' "$out" > "$expected"
    return 0
  fi
  if [ ! -f "$expected" ]; then
    echo "     no expected output — run with SAULE_BLESS=1 to record it"
    return 1
  fi
  if [ "$out" = "$(cat "$expected")" ]; then
    return 0
  fi
  diff "$expected" <(printf '%s\n' "$out") | head -12 | sed 's/^/     /'
  return 1
}

echo '== positive tests =='
for f in tests/*.sau; do
  total=$((total + 1))
  if ! out=$("$SAULE_BIN" run "$f" 2>&1 </dev/null); then
    printf 'FAIL %s\n' "$f"
    echo "$out" | head -5 | sed 's/^/     /'
    failures=$((failures + 1))
    continue
  fi
  if ! output_exempt "$f" && ! d=$(check_output "$f" "$out"); then
    printf 'FAIL %s (output differs)\n' "$f"
    printf '%s\n' "$d"
    failures=$((failures + 1))
    continue
  fi
  printf 'OK   %s\n' "$f"
done

echo
echo '== ui tests (expected to error) =='
for f in tests/ui/*.sau; do
  total=$((total + 1))
  if out=$("$SAULE_BIN" run "$f" 2>&1 </dev/null); then
    printf 'FAIL %s (did not error)\n' "$f"
    failures=$((failures + 1))
    continue
  fi
  # These exist to pin a specific *diagnostic*, so the message itself has to
  # match too — erroring for a different reason is not the same behaviour.
  if ! output_exempt "$f" && ! d=$(check_output "$f" "$out"); then
    printf 'FAIL %s (diagnostic differs)\n' "$f"
    printf '%s\n' "$d"
    failures=$((failures + 1))
    continue
  fi
  printf 'OK   %s\n' "$f"
done

echo
for f in tests/*.sau tests/ui/*.sau; do
  output_exempt "$f" && exempted=$((exempted + 1))
done
if [ "$exempted" -gt 0 ]; then
  echo "note: $exempted fixture(s) exempt from the output check — see output_exempt()"
fi
if [ -n "$SAULE_BLESS" ]; then
  echo "recorded the output of $total fixtures — review the diff"
  exit 0
fi
if [ "$failures" -eq 0 ]; then
  echo "all $total fixtures behaved as expected"
  exit 0
fi
echo "$failures of $total fixtures failed" >&2
exit 1
