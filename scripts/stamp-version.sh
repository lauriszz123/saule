#!/usr/bin/env bash
# Write a resolved version into the files that can't read it at compile time.
#
#   scripts/stamp-version.sh 26.8
#   scripts/stamp-version.sh 26.8 --check    # verify, change nothing
#
# The Rust side needs none of this — `saule-version`'s build script resolves
# the version itself. What's left is the editor plugins, whose manifests are
# read by marketplaces long before any Rust runs:
#
#   * VS Code requires three semver components, so it gets `<year>.<build>.0`.
#   * IntelliJ takes the version verbatim, so it gets `<year>.<build>`.
#
# `--check` is what CI runs on a pull request: it turns "somebody edited one
# manifest and forgot the other" into a failed job instead of a marketplace
# listing that disagrees with `saule --version`.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

version="${1:-}"
mode="${2:-write}"

if [ -z "$version" ]; then
    echo "usage: $(basename "$0") <version> [--check]" >&2
    echo "       e.g. $(basename "$0") 26.8" >&2
    exit 2
fi

if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+$'; then
    echo "error: \`$version\` is not a <year>.<build> version (e.g. 26.8)" >&2
    exit 1
fi

# VS Code's manifest is validated as strict semver; the trailing `.0` is a
# placeholder that never carries meaning.
semver="${version}.0"

status=0

# Replace `pattern` with `replacement` in `file`, or in --check mode report
# whether the expected text is already there.
#
# `expected` is a plain substring searched for verbatim in check mode. It is
# a separate argument rather than reusing `replacement` because a replacement
# may carry regex escapes — `\n` to anchor to the start of a line, say — which
# a literal search would look for as a backslash followed by an `n`.
stamp() {
    local file="$1" label="$2" pattern="$3" replacement="$4" expected="${5:-$4}"
    local path="$ROOT/$file"

    if [ ! -f "$path" ]; then
        echo "error: $file not found" >&2
        status=1
        return
    fi

    if [ "$mode" = "--check" ]; then
        # Passed through the environment, not argv: under `-n` perl treats
        # every argument as a filename to read.
        if EXPECTED="$expected" perl -0777 -ne \
            'exit(index($_, $ENV{EXPECTED}) >= 0 ? 0 : 1)' "$path"; then
            echo "  ok      $file ($label)"
        else
            echo "  STALE   $file ($label) — expected $label $version" >&2
            status=1
        fi
        return
    fi

    perl -0777 -pi -e "s{$pattern}{$replacement}g" "$path"
    echo "  stamped $file ($label)"
}

if [ "$mode" = "--check" ]; then
    echo "Checking plugin manifests against $version…"
else
    echo "Stamping $version into plugin manifests…"
fi

# Anchored to a top-level key: the newline plus exactly two spaces is what
# distinguishes it from a nested `"version"`. package.json has only one today,
# and this pattern would not match a nested one if that ever changed.
stamp editors/vscode/package.json "extension version" \
    '\n  "version": "[0-9][^"]*"' "\n  \"version\": \"$semver\"" \
    "\"version\": \"$semver\""

# The lockfile needs real JSON editing — it pins a `"version"` for every
# dependency, so a textual substitution would rewrite all of them.
"$ROOT/scripts/stamp-npm-version.mjs" "$semver" "$mode" || status=1

stamp editors/intellij/gradle.properties "plugin version" \
    'pluginVersion = [0-9][^\s]*' "pluginVersion = $version"

stamp editors/intellij/src/main/resources/META-INF/plugin.xml "plugin.xml version" \
    '<version>[0-9][^<]*</version>' "<version>$version</version>"

# The IntelliJ new-project wizard writes `min_saule_version` into the
# saule.config it scaffolds, so it has to know the toolchain version too.
stamp editors/intellij/src/main/kotlin/com/saule/lang/project/SauleProjectScaffolder.kt \
    "scaffolded min_saule_version" \
    'const val SAULE_VERSION = "[0-9][^"]*"' "const val SAULE_VERSION = \"$version\""

if [ "$status" -ne 0 ] && [ "$mode" = "--check" ]; then
    echo >&2
    echo "Run: scripts/stamp-version.sh $version" >&2
fi

exit "$status"
