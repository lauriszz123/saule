# Saule installer for Windows.
#
#   irm https://lauriszz123.github.io/saule/install.ps1 | iex
#
# Downloads the release archive from the Saule project's GitLab package
# registry, verifies it against SHA256SUMS, and installs both binaries into
# %USERPROFILE%\.saule\bin.
#
# Because `iex` runs this as an expression rather than as a script file, there
# is no `param()` block and no `$args` — every knob is an environment variable:
#
#   $env:SAULE_VERSION           install this version instead of the latest
#   $env:SAULE_HOME              install root. Used verbatim — it *is* the
#                                directory, not a parent to append `.saule` to.
#   $env:SAULE_NO_MODIFY_PATH=1  install, but leave the user PATH alone.

$ErrorActionPreference = 'Stop'

# Windows PowerShell 5.1 still negotiates TLS 1.0 by default, which gitlab.com
# refuses. PowerShell 7 ignores this because it already defaults higher.
try {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {}

# The one line to change if the project ever moves namespace. The API addresses
# a project by its URL-encoded path; anything a human clicks needs real slashes.
$GitLabHost    = 'https://gitlab.com'
$GitLabProject = 'lauriszz12313/saule'
$PackageName   = 'saule'

$ProjectEnc = $GitLabProject -replace '/', '%2F'
$Api        = "$GitLabHost/api/v4/projects/$ProjectEnc"
$Web        = "$GitLabHost/$GitLabProject"

function Say  { param($m) Write-Host "saule: $m" }
function Fail { param($m) Write-Host "saule: error: $m" -ForegroundColor Red; exit 1 }

# ─── Which build does this machine need? ────────────────────────────────────
$arch = $env:PROCESSOR_ARCHITECTURE
if (-not $arch) { $arch = 'AMD64' }
switch ($arch) {
    'AMD64' { $triple = 'x86_64-pc-windows-msvc' }
    'ARM64' {
        # No native ARM64 build is published yet. Windows on ARM emulates x64
        # transparently, so the x64 archive genuinely works — it is just slower
        # than a native build would be. Say so rather than failing.
        Say 'no native ARM64 build yet; installing the x64 build, which runs under emulation'
        $triple = 'x86_64-pc-windows-msvc'
    }
    default { Fail "unsupported architecture '$arch'. Build from source: $Web" }
}

# ─── Which version? ─────────────────────────────────────────────────────────
if ($env:SAULE_VERSION) {
    $version = $env:SAULE_VERSION -replace '^v', ''
} else {
    try {
        # Invoke-RestMethod parses the JSON, so unlike install.sh there is no
        # hand-rolled parsing to get wrong.
        $latest = Invoke-RestMethod -UseBasicParsing "$Api/releases/permalink/latest"
    } catch {
        Fail "could not reach $Api — is the project public, and are you online?"
    }
    $version = $latest.tag_name -replace '^v', ''
    if (-not $version) { Fail 'could not read a version from the latest release. Set $env:SAULE_VERSION to install a specific one.' }
}

$sauleHome = if ($env:SAULE_HOME) { $env:SAULE_HOME } else { Join-Path $env:USERPROFILE '.saule' }
$archive   = "saule-$version-$triple.zip"
$base      = "$Api/packages/generic/$PackageName/$version"

Say "installing Saule $version for $triple"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("saule-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    Say "downloading $archive"
    try {
        Invoke-WebRequest -UseBasicParsing "$base/$archive" -OutFile (Join-Path $tmp $archive)
    } catch {
        Fail "could not download $archive.`n    Is $version a published release? See $Web/-/releases"
    }
    try {
        Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS')
    } catch {
        Fail 'could not download SHA256SUMS — refusing to install unverified binaries'
    }

    # ─── Verify ─────────────────────────────────────────────────────────────
    # Never skipped: `irm | iex` installing an unverified binary is the single
    # riskiest thing this script could do.
    #
    # SHA256SUMS is written by `sha256sum` on Linux, so it lists every
    # platform's archive as "<hash>  <name>". Only our own line matters.
    $line = Get-Content (Join-Path $tmp 'SHA256SUMS') |
        Where-Object { $_ -match "\s\*?$([regex]::Escape($archive))\s*$" } |
        Select-Object -First 1
    if (-not $line) { Fail "$archive is not listed in SHA256SUMS — refusing to install" }

    $expected = ($line -split '\s+')[0]
    $actual   = (Get-FileHash -Algorithm SHA256 -Path (Join-Path $tmp $archive)).Hash
    if ($actual -ne $expected.ToUpperInvariant() -and $actual -ne $expected) {
        Fail "checksum mismatch for $archive — refusing to install`n    expected $expected`n    got      $actual"
    }
    Say 'checksum ok'

    # ─── Install ────────────────────────────────────────────────────────────
    Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp -Force
    $unpacked = Join-Path $tmp "saule-$version-$triple"
    if (-not (Test-Path $unpacked)) { Fail "$archive did not contain saule-$version-$triple" }

    # The directories the toolchain expects to exist. `native_manifests` and
    # `native_packages` are what dynamic_packages/discovery.rs actually reads.
    foreach ($d in 'bin', 'native_manifests', 'native_packages', 'tmp') {
        New-Item -ItemType Directory -Force -Path (Join-Path $sauleHome $d) | Out-Null
    }

    $bin = Join-Path $sauleHome 'bin'
    # A running saule.exe cannot be overwritten, which is the normal way a
    # re-install fails on Windows and produces a thoroughly unhelpful error.
    foreach ($exe in 'saule.exe', 'saule-lsp.exe') {
        $dest = Join-Path $bin $exe
        if (Test-Path $dest) {
            try { Move-Item $dest "$dest.old" -Force; Remove-Item "$dest.old" -Force -EA SilentlyContinue }
            catch { Fail "$dest is in use. Close any running saule or editor language server and try again." }
        }
        Copy-Item (Join-Path $unpacked $exe) $dest -Force
    }
    Say "installed to $bin"

    # Both binaries matter. `saule-lsp` is the language server every editor
    # plugin uses; without it on PATH you get syntax highlighting and
    # indentation but no diagnostics, hover or formatting.
    $got = & (Join-Path $bin 'saule.exe') --version
    if ($got -ne "saule $version") { Fail "installed binary reports '$got', expected 'saule $version'" }

    # ─── PATH ───────────────────────────────────────────────────────────────
    if ($env:SAULE_NO_MODIFY_PATH -eq '1') {
        Say "leaving PATH alone as asked; add $bin to it yourself"
    } else {
        # The *user* PATH, read back from the registry rather than from
        # $env:PATH — the process copy is the user and machine values already
        # concatenated, and writing that back would duplicate every system
        # entry into the user's own PATH permanently.
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        if (-not $userPath) { $userPath = '' }
        $entries = $userPath -split ';' | Where-Object { $_ -ne '' }
        if ($entries -contains $bin) {
            Say 'already on PATH'
        } else {
            [Environment]::SetEnvironmentVariable('Path', (@($bin) + $entries) -join ';', 'User')
            Say 'added to your user PATH'
        }
        # So the current session works without being restarted.
        $env:Path = "$bin;$env:Path"
    }

    Write-Host ''
    Say "$got is installed."
    Write-Host ''
    Write-Host '  Open a new terminal, then run:  saule --version'
    Write-Host ''
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
