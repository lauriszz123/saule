#!/usr/bin/env bash
# Run every `.sau` fixture through the debug build.
#
#   tests/run/<area>/*.sau      must run and exit 0
#   tests/errors/<area>/*.sau   must fail — each one exists to pin a specific
#                               diagnostic
#
# **Exit status is all that is checked.** The recorded outputs this suite used
# to compare against were removed deliberately; what a fixture prints, and
# what diagnostic an error fixture produces, is no longer gated. So a fixture
# that fails for the wrong reason passes here, and a change that prints the
# wrong value while still exiting 0 passes too — `tests/errors/README.md` has
# the two cases where that happened before. Read a fixture's output yourself
# when you change one.
#
# Exits non-zero if any fixture is on the wrong side of either line, so CI
# can gate on it. `SAULE_BIN` overrides the binary (e.g. to test a release
# build).
#
# `--list` prints the corpus instead of running it: every fixture grouped by
# area, with the first sentence of its own leading comment as the description.
# That makes the scope of the suite readable without opening 277 files, and it
# reads the fixtures themselves rather than a table that would go stale.
# `--list <area>` narrows to one area (`generics`, `errors/classes`, …).
SAULE_BIN="${SAULE_BIN:-}"

# Default to whichever of the two names exists: Windows builds `saule.exe`.
if [ -z "$SAULE_BIN" ]; then
  if [ -x ./target/debug/saule ]; then
    SAULE_BIN=./target/debug/saule
  else
    SAULE_BIN=./target/debug/saule.exe
  fi
fi

# ── `--list` ────────────────────────────────────────────────────────────────
# Reads each fixture's leading `--` comment block and prints its first
# sentence. A fixture with no comment is listed with a `?`, which is a gap
# worth seeing rather than hiding.
fixture_summary() {
  awk '
    # Stop at the first line that is not a `--` comment: the description is
    # the block at the top of the file, not every comment in it.
    !/^--/ { exit }
    {
      line = $0
      sub(/^--+[ \t]*/, "", line)
      if (line == "") { if (text != "") exit; else next }
      text = (text == "" ? line : text " " line)
      # One sentence is enough for a listing, and the first one is the one
      # fixtures are written to lead with.
      if (text ~ /\. /) { sub(/\. .*/, ".", text); exit }
      if (text ~ /\.$/) { exit }
    }
    END {
      gsub(/[ \t]+/, " ", text)
      # Cut on a word boundary, so a truncated line still reads.
      if (length(text) > 72) {
        text = substr(text, 1, 69)
        sub(/[ \t][^ \t]*$/, "", text)
        text = text " ..."
      }
      print (text == "" ? "?" : text)
    }
  ' "$1"
}

list_corpus() {
  local want="${1:-}" area last=""
  for f in tests/run/*/*.sau tests/errors/*/*.sau; do
    area="$(dirname "${f#tests/}")"
    # `--list generics` matches both trees; `--list errors/classes` one.
    if [ -n "$want" ] && [ "${area#*/}" != "$want" ] && [ "$area" != "$want" ]; then
      continue
    fi
    if [ "$area" != "$last" ]; then
      [ -n "$last" ] && echo
      printf '%s (%s)\n' "$area" "$(ls "tests/$area"/*.sau | wc -l | tr -d ' ')"
      last="$area"
    fi
    printf '  %-38s %s\n' "$(basename "$f" .sau)" "$(fixture_summary "$f")"
  done
  [ -n "$last" ] || { echo "no such area: $want" >&2; return 1; }
}

if [ "${1:-}" = "--list" ]; then
  list_corpus "${2:-}"
  exit $?
fi

if [ ! -x "$SAULE_BIN" ]; then
  echo "error: $SAULE_BIN not found — run 'cargo build -p saule-cli' first" >&2
  exit 1
fi

failures=0
total=0

# Fixtures that cannot run on this platform at all, each with a reason.
# Counted at the end for the same reason as the exemptions above.
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) on_windows=1 ;;
  *) on_windows= ;;
esac
platform_skip() {
  [ -n "$on_windows" ] || return 1
  case "$1" in
    # Spawns `sh`, `cat` and `printf` and reads `/dev/zero` — a Unix
    # userland. `crates/saule-cli/tests/spawn.rs` pins the same behaviour
    # on every platform, Windows included, with `saule` as the child.
    tests/run/stdlib/spawn.sau) return 0 ;;
    *) return 1 ;;
  esac
}
skipped=0

echo '== run tests =='
for f in tests/run/*/*.sau; do
  if platform_skip "$f"; then
    printf 'SKIP %s (not on this platform)\n' "$f"
    skipped=$((skipped + 1))
    continue
  fi
  total=$((total + 1))
  if ! out=$("$SAULE_BIN" run "$f" 2>&1 </dev/null); then
    printf 'FAIL %s\n' "$f"
    echo "$out" | head -5 | sed 's/^/     /'
    failures=$((failures + 1))
    continue
  fi
  printf 'OK   %s\n' "$f"
done

echo
echo '== error tests (expected to fail) =='
for f in tests/errors/*/*.sau; do
  total=$((total + 1))
  # Only *that* it errors, not how: the diagnostic each fixture is named for
  # is no longer compared against anything.
  if out=$("$SAULE_BIN" run "$f" 2>&1 </dev/null); then
    printf 'FAIL %s (did not error)\n' "$f"
    failures=$((failures + 1))
    continue
  fi
  printf 'OK   %s\n' "$f"
done

echo
if [ "$skipped" -gt 0 ]; then
  echo "note: $skipped fixture(s) skipped on this platform — see platform_skip()"
fi
if [ "$failures" -eq 0 ]; then
  echo "all $total fixtures exited as expected"
  exit 0
fi
echo "$failures of $total fixtures failed" >&2
exit 1
