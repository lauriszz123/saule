# Put the Saule toolchain on PATH for PowerShell and cmd.
#
# The Windows counterpart to scripts/install_path.sh, with one deliberate
# difference. The Unix script symlinks the binaries into ~/.local/bin; here the
# release directory itself goes on PATH, because Windows symlinks need either
# Developer Mode or an elevated shell and a plain copy would go stale on every
# rebuild. Pointing at `target\release` means `cargo build --release` is picked
# up with no reinstall step.
#
# The tradeoff: `cargo clean` deletes the directory this PATH entry names, and
# both commands vanish until the next release build. Re-run this script (or
# just rebuild) — do not go editing PATH again.
#
# Both binaries are installed. `saule-lsp` matters as much as `saule`: the
# editor plugins look for the language server in the project's `target/`
# directory first and fall back to PATH, so without it on PATH the LSP only
# works inside this repo — in any other project you get syntax highlighting
# and indentation (both client-side) but no formatting, hover or diagnostics.
#
# Usage:
#   powershell -File scripts\install_windows_path.ps1
#   powershell -File scripts\install_windows_path.ps1 -Force      # kill a running saule-lsp first
#   powershell -File scripts\install_windows_path.ps1 -SkipBuild  # wire PATH to what is already built
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$release = Join-Path $repo 'target\release'
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = 'cargo' }

$bins = @('saule', 'saule-lsp')

$needsBuild = $false
foreach ($bin in $bins) {
    if (-not (Test-Path (Join-Path $release "$bin.exe"))) { $needsBuild = $true }
}

if ($needsBuild -and $SkipBuild) {
    throw "release binaries missing from $release and -SkipBuild was given"
}

if (-not $SkipBuild) {
    # A running saule-lsp.exe is locked by Windows, and an editor with the
    # Saule plugin open respawns it within milliseconds of being killed, so a
    # single Stop-Process loses the race against cargo's unlink. Kill and
    # delete in a tight loop instead. Opt-in: this terminates a language
    # server the user may be actively using.
    if ($Force) {
        $lsp = Join-Path $release 'saule-lsp.exe'
        for ($i = 0; $i -lt 15; $i++) {
            Get-Process saule-lsp -ErrorAction SilentlyContinue |
                Stop-Process -Force -ErrorAction SilentlyContinue
            if (-not (Test-Path $lsp)) { break }
            try { Remove-Item $lsp -Force -ErrorAction Stop; break } catch { }
        }
    }

    Write-Host 'building saule and saule-lsp (release)...'
    & $cargo build --release -p saule-cli -p saule-lsp --manifest-path (Join-Path $repo 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) {
        if (-not $Force) {
            Write-Host ''
            Write-Host 'if this failed with "Access is denied" on saule-lsp.exe, the language'
            Write-Host 'server is running - close the editor or re-run with -Force.'
        }
        throw 'build failed'
    }
}

foreach ($bin in $bins) {
    $exe = Join-Path $release "$bin.exe"
    if (-not (Test-Path $exe)) { throw "$exe not found after build" }
}

# Compare normalized (trailing separator stripped, case-insensitive) so a
# re-run does not append a second near-identical entry.
$normalized = $release.TrimEnd('\')
$current = [Environment]::GetEnvironmentVariable('PATH', 'User')
$entries = @($current -split ';' | Where-Object { $_ })
$present = $entries | Where-Object { $_.TrimEnd('\') -ieq $normalized }

if ($present) {
    Write-Host "PATH entry already present: $normalized"
} else {
    # SetEnvironmentVariable writes REG_SZ, which would stop any %VAR% in an
    # existing entry from expanding. Refuse rather than quietly break PATH.
    if ($current -match '%') {
        throw @"
User PATH contains a %VAR% placeholder; writing it back would freeze the
expansion. Add this entry by hand instead:
  $normalized
"@
    }
    [Environment]::SetEnvironmentVariable(
        'PATH', ($current.TrimEnd(';') + ';' + $normalized), 'User')
    Write-Host "added to User PATH: $normalized"
}

# Make it work in this session too, so the caller can test without reopening.
if (-not (@($env:PATH -split ';') | Where-Object { $_.TrimEnd('\') -ieq $normalized })) {
    $env:PATH = "$env:PATH;$normalized"
}

# Already-running apps cache their environment. Broadcasting WM_SETTINGCHANGE
# lets Explorer pick the change up, so shells launched from it afterwards
# inherit the new PATH. Best effort - nothing here depends on it.
try {
    if (-not ('Win32.NativeMethods' -as [type])) {
        Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(
    IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
    }
    $HWND_BROADCAST = [IntPtr]0xffff
    $WM_SETTINGCHANGE = 0x1a
    $SMTO_ABORTIFHUNG = 0x2
    $unused = [UIntPtr]::Zero
    [void][Win32.NativeMethods]::SendMessageTimeout(
        $HWND_BROADCAST, $WM_SETTINGCHANGE, [UIntPtr]::Zero, 'Environment',
        $SMTO_ABORTIFHUNG, 5000, [ref]$unused)
} catch { }

Write-Host ''
Write-Host 'installed:'
foreach ($bin in $bins) { Write-Host "  $(Join-Path $release "$bin.exe")" }
Write-Host ''
Write-Host 'test in a NEW terminal with:'
Write-Host '  saule --version'
Write-Host '  saule-lsp --version'
Write-Host ''
Write-Host 'note: a shell open before this ran keeps its old PATH - Windows copies'
Write-Host '      the environment at process start and never re-reads it. The same'
Write-Host '      applies to IntelliJ: restart it so the plugin inherits the change,'
Write-Host '      or set the server path explicitly in'
Write-Host '      Settings > Languages & Frameworks > Saule:'
Write-Host "        $(Join-Path $release 'saule-lsp.exe')"
