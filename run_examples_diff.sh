#!/usr/bin/env bash
#
# Differential harness over `examples/` — VM_DESIGN.md §23.2, and the Phase 3
# exit criterion `run_tests.sh` does not cover.
#
# `run_tests.sh` compares the two engines on `tests/*.sau`: small,
# single-file, side-effect-free fixtures. This runs the *example projects*
# instead — multi-module programs with imports, file IO and real
# dependencies. That difference is the point. Every silent divergence this
# project has found so far came from running code shaped like a program
# rather than like a fixture:
#
#   * a `match` guard that ran an arm whose pattern did not match — no
#     fixture pairs a literal pattern with a guard;
#   * a cross-module `self.super()` that recursed forever — no fixture has
#     two modules;
#   * an enum variant's `.value` reading `nil` — the fixture that would have
#     caught it was falling back.
#
# Usage:
#   SAULE_BIN=./target/debug/saule.exe bash run_examples_diff.sh
#
# Exit status is non-zero when the engines disagree on any project.

set -u
# Default to whichever of the two names exists, the way `run_tests.sh` does,
# rather than hard-coding `.exe`. The guard below is the load-bearing half:
# a binary that is not there fails *identically* under both engines, and this
# harness would report that as "every project agrees" — a green run that
# tested nothing at all.
if [ -z "${SAULE_BIN:-}" ]; then
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
TIMEOUT="${SAULE_EXAMPLE_TIMEOUT:-20}"

# GNU `timeout` is not on a stock macOS, and without a shim every project
# here fails identically — which this harness reports as "9 of 9 projects
# disagreed", a divergence that is not real. Prefer coreutils when it is
# installed, and otherwise run the command under a watchdog.
#
# The watchdog's output goes to /dev/null deliberately: these calls run
# inside `$(...)`, and a background process holding the capture pipe open
# would make every project wait the full timeout before the substitution
# returned.
if command -v timeout >/dev/null 2>&1; then
  run_limited() { timeout "$@"; }
elif command -v gtimeout >/dev/null 2>&1; then
  run_limited() { gtimeout "$@"; }
else
  run_limited() {
    local secs=$1
    shift
    "$@" &
    local pid=$!
    ( sleep "$secs"; kill -9 "$pid" 2>/dev/null ) >/dev/null 2>&1 &
    local watcher=$!
    wait "$pid" 2>/dev/null
    local rc=$?
    kill "$watcher" 2>/dev/null
    wait "$watcher" 2>/dev/null
    return $rc
  }
fi

# The VM declining to compile something is a designed outcome, not a
# behavioural difference — same rule as `run_tests.sh`.
strip_notes() {
  grep -vE '^note: the bytecode compiler (does not handle|could not build) ' || true
}

# Projects this harness cannot compare, each with a reason. Counted and
# printed at the end, because an exclusion you cannot see is just a test you
# stopped running.
skip_reason() {
  case "$1" in
    # Both open a window and loop until it is closed, so neither has a
    # terminating run to compare — they hang identically under *both*
    # engines, which is a property of the program, not a divergence.
    # Covered by the UI Project's own manual workflow instead.
    */UI\ Project) echo "interactive: opens a window and loops until closed" ;;
    */toying) echo "interactive: opens a window and loops until closed" ;;
    # Libraries have no entry point, so `saule run` refuses them — identically
    # under both engines. Left unskipped they would be counted as two projects
    # that agree, which is the green-run-that-tested-nothing this harness
    # exists to avoid. `uikit` is exercised through the UI Project instead.
    */uikit) echo "library: no entry point to run" ;;
    */markdown) echo "library: no entry point to run" ;;
    */md-viewer) echo "interactive: opens a window and loops until closed" ;;
    *) return 1 ;;
  esac
}

# Some examples write files. Running the same project twice must start from
# the same state or the second run reports different output for a reason
# that has nothing to do with the engine.
snapshot() {
  local d="$1"
  rm -rf "$SNAP"
  cp -r "$d" "$SNAP" 2>/dev/null || true
}
restore() {
  local d="$1"
  rm -rf "$d"
  cp -r "$SNAP" "$d" 2>/dev/null || true
}

SNAP="$(mktemp -d)/snap"
total=0
failures=0
skipped=0
fellback=0

echo "== examples/ under both engines =="
while IFS= read -r cfg; do
  d=$(dirname "$cfg")
  total=$((total + 1))

  if reason=$(skip_reason "$d"); then
    printf 'SKIP %-24s %s\n' "$(basename "$d")" "$reason"
    skipped=$((skipped + 1))
    continue
  fi

  snapshot "$d"
  interp=$(run_limited "$TIMEOUT" env SAULE_ENGINE=interp "$SAULE_BIN" run "$d" 2>&1)
  ic=$?
  restore "$d"
  raw_vm=$(run_limited "$TIMEOUT" env SAULE_ENGINE=vm "$SAULE_BIN" run "$d" 2>&1)
  vc=$?
  restore "$d"

  # Whether the VM actually compiled it, before the note is stripped — a
  # project that fell back proves the engines agree, not that the VM ran it.
  if printf '%s' "$raw_vm" | grep -q '^note: the bytecode compiler '; then
    fellback=$((fellback + 1))
    mark="(fallback)"
  else
    mark=""
  fi

  i=$(printf '%s' "$interp" | strip_notes)
  v=$(printf '%s' "$raw_vm" | strip_notes)

  if [ "$ic" -ne "$vc" ]; then
    printf 'FAIL %-24s exit status %s vs %s\n' "$(basename "$d")" "$ic" "$vc"
    failures=$((failures + 1))
    continue
  fi
  if [ "$i" != "$v" ]; then
    printf 'FAIL %-24s engines disagree\n' "$(basename "$d")"
    diff <(printf '%s\n' "$i") <(printf '%s\n' "$v") | head -14 | sed 's/^/     /'
    failures=$((failures + 1))
    continue
  fi
  printf 'OK   %-24s %s\n' "$(basename "$d")" "$mark"
done < <(find examples -name saule.config | sort)

rm -rf "$(dirname "$SNAP")"

echo
compared=$((total - skipped))
echo "$compared of $total projects compared; $fellback fell back to the tree-walker"
if [ "$skipped" -gt 0 ]; then
  echo "note: $skipped project(s) skipped — see skip_reason()"
fi
if [ "$failures" -eq 0 ]; then
  echo "both engines agree on every project compared"
  exit 0
fi
echo "$failures of $compared projects disagreed"
exit 1
