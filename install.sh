#!/bin/sh
# Saule installer.
#
#   curl -fsSL https://lauriszz123.github.io/saule/install.sh | sh
#
# Downloads the release archive for this machine from the Saule project's
# GitHub releases, verifies it against SHA256SUMS, and installs both binaries
# into ~/.saule/bin.
#
# POSIX sh throughout — no bash, no jq. It runs under dash, busybox ash and
# whatever /bin/sh is on macOS, because `curl | sh` picks the shell and the
# script does not get a say.
#
# Environment:
#   SAULE_VERSION   install this version instead of the latest, e.g. 26.7
#   SAULE_HOME      install root. Used verbatim — it *is* the directory, not
#                   a parent to append `.saule` to. Defaults to ~/.saule.
#   SAULE_NO_MODIFY_PATH=1
#                   install the binaries but leave shell rc files alone.

set -eu

# The one line to change if the project ever moves namespace. Release assets
# hang off the web host, not the API — `github.com/.../releases/download/...`
# is a plain anonymous download with no rate limit on it.
GITHUB_HOST="https://github.com"
GITHUB_REPO="lauriszz123/saule"

WEB="$GITHUB_HOST/$GITHUB_REPO"
API="https://api.github.com/repos/$GITHUB_REPO"

say() { printf 'saule: %s\n' "$*"; }
err() { printf 'saule: error: %s\n' "$*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || err "\`$1\` is required but was not found on PATH"
}

# ─── Downloader ─────────────────────────────────────────────────────────────
# curl on most systems, wget on the minimal containers that ship without it.
# Both are told to fail loudly on a 404: the default for wget is to happily
# write the error page to disk, which would then fail checksum verification
# with a baffling message instead of a clear one.
#
# `fetch_final_url` reports where a redirect landed without downloading the
# body — see `resolve_tag`. Only curl can do it portably, so the wget branch
# declines and the caller falls back to the API.
if command -v curl >/dev/null 2>&1; then
    fetch()  { curl -fsSL --proto '=https' --tlsv1.2 "$1" -o "$2"; }
    fetch_stdout() { curl -fsSL --proto '=https' --tlsv1.2 "$1"; }
    fetch_final_url() {
        curl -fsSL --proto '=https' --tlsv1.2 -o /dev/null -w '%{url_effective}' "$1"
    }
elif command -v wget >/dev/null 2>&1; then
    fetch()  { wget -q "$1" -O "$2"; }
    fetch_stdout() { wget -q "$1" -O -; }
    fetch_final_url() { return 1; }
else
    err "neither curl nor wget is available"
fi

need uname
need tar
need mkdir
need mv

# ─── Which build does this machine need? ────────────────────────────────────
detect_triple() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$arch" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) err "unsupported architecture \`$arch\`. Build from source: $WEB" ;;
    esac

    case "$os" in
        Darwin)
            echo "$arch-apple-darwin"
            ;;
        Linux)
            # A musl-only box given a glibc binary gets "no such file or
            # directory" from the loader, which is one of the least helpful
            # error messages in computing. Detect it up front instead.
            #
            # `ldd --version` writes to stderr on glibc and exits non-zero on
            # musl, so both streams are captured and the status ignored.
            if [ "$arch" = "aarch64" ]; then
                # Only a glibc aarch64 archive is published today. Say so
                # plainly rather than shipping a binary that cannot start.
                if ! (ldd --version 2>&1 || true) | grep -qi 'glibc\|gnu libc'; then
                    err "aarch64 Linux is only published as a glibc build, and this system looks like musl. Build from source: $WEB"
                fi
                echo "aarch64-unknown-linux-gnu"
            elif (ldd --version 2>&1 || true) | grep -qi 'glibc\|gnu libc'; then
                echo "x86_64-unknown-linux-gnu"
            else
                echo "x86_64-unknown-linux-musl"
            fi
            ;;
        MINGW* | MSYS* | CYGWIN*)
            err "on Windows use install.ps1 instead:
    irm https://lauriszz123.github.io/saule/install.ps1 | iex"
            ;;
        *)
            err "unsupported operating system \`$os\`"
            ;;
    esac
}

# ─── Which version? ─────────────────────────────────────────────────────────
# Releases are tagged `v26.7`, while the archives inside them carry the bare
# `26.7`. Both spellings are needed, so this resolves the tag and `main`
# strips the `v` off it.
resolve_tag() {
    if [ -n "${SAULE_VERSION:-}" ]; then
        printf 'v%s' "${SAULE_VERSION#v}"
        return
    fi

    # `/releases/latest` is a redirect to `/releases/tag/vX.Y`, so the tag can
    # be read straight out of the URL it settles on. That is deliberately not
    # api.github.com: the API allows 60 unauthenticated requests an hour per
    # IP, and a whole office behind one NAT can spend them between them — at
    # which point the installer looks broken to everybody there. The redirect
    # is served by the web host and is not rate limited.
    url="$(fetch_final_url "$WEB/releases/latest" 2>/dev/null || true)"
    tag="${url##*/tag/}"
    case "$tag" in
        v[0-9]*)
            printf '%s' "$tag"
            return
            ;;
    esac

    # wget cannot report where a redirect went, so it pays the API's limit.
    # sed rather than jq, so the installer needs nothing but a downloader.
    json="$(fetch_stdout "$API/releases/latest" 2>/dev/null || true)"
    [ -n "$json" ] || err "could not resolve the latest release from $WEB/releases.
    Are you online, and does the project have a published release yet?
    Set SAULE_VERSION to install a specific one."

    tag="$(printf '%s' "$json" \
        | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
        | head -1)"
    [ -n "$tag" ] || err "could not read a tag from the latest release. Set SAULE_VERSION to install a specific one."
    printf '%s' "$tag"
}

# ─── Verification ───────────────────────────────────────────────────────────
# Never skipped, never silently degraded: `curl | sh` installing an unverified
# binary is the single riskiest thing this script could do.
verify() {
    archive="$1"
    sums="$2"

    # `-c` reads the full SHA256SUMS, which lists every platform's archive.
    # Filtering to our own filename first keeps the other five from being
    # reported as missing.
    want="$(grep " [ *]\{0,1\}$archive\$" "$sums" || true)"
    [ -n "$want" ] || err "$archive is not listed in SHA256SUMS — refusing to install"
    printf '%s\n' "$want" > expected.sha256

    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c expected.sha256 >/dev/null \
            || err "checksum mismatch for $archive — refusing to install"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 -c expected.sha256 >/dev/null \
            || err "checksum mismatch for $archive — refusing to install"
    else
        err "neither sha256sum nor shasum is available, so the download cannot be verified.
    Install coreutils (or perl) and try again. This check is not optional."
    fi
    say "checksum ok"
}

# ─── PATH ───────────────────────────────────────────────────────────────────
# One sourceable snippet, referenced from each rc file — rather than the same
# `export PATH` line duplicated into three of them, which is what the old
# scripts/install_path.sh did and what made it impossible to change later.
write_env_file() {
    home="$1"
    cat > "$home/env" <<ENVEOF
# Added by the Saule installer. Prepends Saule's bin directory to PATH,
# but only when it is not already there — so re-sourcing is harmless and
# repeated logins do not grow PATH without bound.
case ":\${PATH}:" in
    *:"$home/bin":*) ;;
    *) export PATH="$home/bin:\$PATH" ;;
esac
ENVEOF
}

add_to_rc() {
    rc="$1"
    line="$2"
    [ -f "$rc" ] || return 0
    # -F because the line contains `$` and `.`, and this is a literal match.
    if grep -Fqs "$line" "$rc"; then
        say "already in ${rc#"$HOME"/}"
        return 0
    fi
    printf '\n%s\n' "$line" >> "$rc"
    say "added to ${rc#"$HOME"/}"
}

# ─── Install ────────────────────────────────────────────────────────────────
main() {
    triple="$(detect_triple)"
    tag="$(resolve_tag)"
    version="${tag#v}"
    saule_home="${SAULE_HOME:-$HOME/.saule}"

    archive="saule-$version-$triple.tar.gz"
    base="$WEB/releases/download/$tag"

    say "installing Saule $version for $triple"

    tmp="$(mktemp -d "${TMPDIR:-/tmp}/saule-install.XXXXXX")" \
        || err "could not create a temporary directory"
    # Runs on normal exit and on interrupt, so a failed install leaves nothing
    # behind in /tmp.
    trap 'rm -rf "$tmp"' EXIT INT TERM
    cd "$tmp"

    say "downloading $archive"
    fetch "$base/$archive" "$archive" \
        || err "could not download $archive.
    Is $version a published release for $triple? See $WEB/releases"
    fetch "$base/SHA256SUMS" SHA256SUMS \
        || err "could not download SHA256SUMS — refusing to install unverified binaries"

    verify "$archive" SHA256SUMS

    tar xzf "$archive" || err "could not unpack $archive"
    unpacked="saule-$version-$triple"
    [ -d "$unpacked" ] || err "$archive did not contain $unpacked"

    # The directories the toolchain expects to exist. `native_manifests` and
    # `native_packages` are what dynamic_packages/discovery.rs actually reads.
    mkdir -p "$saule_home/bin" "$saule_home/native_manifests" \
             "$saule_home/native_packages" "$saule_home/tmp"

    # Staged inside SAULE_HOME rather than moved straight from /tmp, so the
    # final step is a rename within one filesystem. An interrupted install then
    # cannot leave a half-written `saule` on PATH.
    stage="$saule_home/tmp/$$"
    rm -rf "$stage"
    mkdir -p "$stage"
    cp "$unpacked/saule" "$unpacked/saule-lsp" "$stage/"
    chmod +x "$stage/saule" "$stage/saule-lsp"
    mv -f "$stage/saule" "$saule_home/bin/saule"
    mv -f "$stage/saule-lsp" "$saule_home/bin/saule-lsp"
    rmdir "$stage" 2>/dev/null || true

    say "installed to $saule_home/bin"

    # Both binaries matter. `saule-lsp` is the language server every editor
    # plugin uses; without it on PATH you get syntax highlighting and
    # indentation but no diagnostics, hover or formatting.
    got="$("$saule_home/bin/saule" --version 2>/dev/null || true)"
    [ "$got" = "saule $version" ] \
        || err "installed binary reports \`$got\`, expected \`saule $version\`"

    if [ "${SAULE_NO_MODIFY_PATH:-}" = "1" ]; then
        say "leaving PATH alone as asked; add $saule_home/bin to it yourself"
    else
        write_env_file "$saule_home"
        line=". \"$saule_home/env\""
        add_to_rc "$HOME/.zshrc"   "$line"
        add_to_rc "$HOME/.bashrc"  "$line"
        add_to_rc "$HOME/.profile" "$line"
        # fish is not POSIX and cannot source the snippet above, so it gets its
        # own idempotent one-liner.
        if [ -d "$HOME/.config/fish" ]; then
            mkdir -p "$HOME/.config/fish/conf.d"
            printf 'fish_add_path -g %s/bin\n' "$saule_home" \
                > "$HOME/.config/fish/conf.d/saule.fish"
            say "added to .config/fish/conf.d/saule.fish"
        fi
    fi

    printf '\n'
    say "$got is installed."
    printf '\n'
    printf '  Start a new shell, or run:  exec $SHELL -l\n'
    printf '  Then:                       saule --version\n'
    printf '\n'
}

main "$@"
