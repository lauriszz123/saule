#!/usr/bin/env bash
#
# Runs every project under `examples/` and compares what it prints — and its
# exit status — with `tests/examples/<project>.out`.
#
# `run_tests.sh` covers `tests/*.sau`: small, single-file, side-effect-free
# fixtures. This runs the *example projects* instead — multi-module programs
# with imports, file IO and real dependencies. That difference is the point.
# Most of the silent bugs this project has found came from running code
# shaped like a program rather than like a fixture:
#
#   * a `match` guard that ran an arm whose pattern did not match — no
#     fixture pairs a literal pattern with a guard;
#   * a cross-module `self.super()` that recursed forever — no fixture has
#     two modules;
#   * an enum variant's `.value` reading `nil`.
#
# The expected outputs were recorded from the tree-walking interpreter, the
# engine that defined the language until it was removed; each was also the
# bytecode VM's output then.
#
# Usage:
#   SAULE_BIN=./target/debug/saule.exe bash run_examples.sh
#   SAULE_BLESS=1 bash run_examples.sh    # re-record; review the diff
#
# Exit status is non-zero when any project differs from its recording.

set -u
# Default to whichever of the two names exists, the way `run_tests.sh` does,
# rather than hard-coding `.exe`.
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
SAULE_BLESS="${SAULE_BLESS:-}"

# GNU `timeout` is not on a stock macOS, and without a shim every project
# here fails identically. Prefer coreutils when it is installed, and
# otherwise run the command under a watchdog.
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

# Projects this harness cannot run, each with a reason. Counted and printed
# at the end, because an exclusion you cannot see is just a test you stopped
# running.
skip_reason() {
  case "$1" in
    # Both open a window and loop until it is closed, so neither has a
    # terminating run to compare. Covered by the UI Project's own manual
    # workflow instead.
    */UI\ Project) echo "interactive: opens a window and loops until closed" ;;
    */toying) echo "interactive: opens a window and loops until closed" ;;
    # Libraries have no entry point, so `saule run` refuses them — which
    # `json` is kept to pin, since it has a recording. `uikit` is exercised
    # through the UI Project instead.
    */uikit) echo "library: no entry point to run" ;;
    */markdown) echo "library: no entry point to run" ;;
    */md-viewer) echo "interactive: opens a window and loops until closed" ;;
    *) return 1 ;;
  esac
}

# Some examples write files. Each run must start from the same state or it
# reports different output for a reason that has nothing to do with the
# program — so the project is restored after it runs.
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

echo "== examples/ =="
while IFS= read -r cfg; do
  d=$(dirname "$cfg")
  name=$(basename "$d")
  total=$((total + 1))

  if reason=$(skip_reason "$d"); then
    printf 'SKIP %-24s %s\n' "$name" "$reason"
    skipped=$((skipped + 1))
    continue
  fi

  snapshot "$d"
  out=$(run_limited "$TIMEOUT" "$SAULE_BIN" run "$d" 2>&1 </dev/null)
  rc=$?
  restore "$d"
  got=$(printf '%s\n[exit %s]' "$out" "$rc")

  expected="tests/examples/$name.out"
  if [ -n "$SAULE_BLESS" ]; then
    printf '%s\n' "$got" > "$expected"
    printf 'REC  %s\n' "$name"
    continue
  fi
  if [ ! -f "$expected" ]; then
    printf 'FAIL %-24s no recording — run with SAULE_BLESS=1 to make one\n' "$name"
    failures=$((failures + 1))
    continue
  fi
  if [ "$got" != "$(cat "$expected")" ]; then
    printf 'FAIL %-24s output differs\n' "$name"
    diff "$expected" <(printf '%s\n' "$got") | head -14 | sed 's/^/     /'
    failures=$((failures + 1))
    continue
  fi
  printf 'OK   %s\n' "$name"
done < <(find examples -name saule.config | sort)

rm -rf "$(dirname "$SNAP")"

echo
ran=$((total - skipped))
if [ "$skipped" -gt 0 ]; then
  echo "note: $skipped project(s) skipped — see skip_reason()"
fi
if [ -n "$SAULE_BLESS" ]; then
  echo "recorded $ran projects — review the diff"
  exit 0
fi
if [ "$failures" -eq 0 ]; then
  echo "all $ran projects ran as recorded"
  exit 0
fi
echo "$failures of $ran projects differed"
exit 1
