# Install the `engine` native package into Windows' %USERPROFILE%\.saule.
#
# Unlike the Unix scripts, Cargo already names the Windows cdylib
# `saule_engine_lib.dll` — matching the manifest's `binary` list — so nothing
# is renamed on copy. The manifest (`engine.toml`) is generated from the
# crate's `#[saule_export]` declarations by the `gen-manifest` binary, which is
# built alongside the library.
#
# Usage:
#   pwsh -File scripts\install_windows.ps1            # build, generate, install
#   pwsh -File scripts\install_windows.ps1 -SkipBuild # install what's there
[CmdletBinding()]
param(
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$release = Join-Path $repo 'target\release'
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = 'cargo' }

if (-not $SkipBuild) {
    Write-Host 'building saule-engine-lib (release)...'
    & $cargo build --release -p saule-engine-lib --manifest-path (Join-Path $repo 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'build failed' }

    # Regenerate the canonical manifest from the exports, then build once more
    # so build.rs copies the refreshed engine.toml next to the artifact.
    Write-Host 'regenerating engine.toml from #[saule_export] declarations...'
    & (Join-Path $release 'gen-manifest.exe') (Join-Path $repo 'crates\saule-engine-lib\engine.toml')
    if ($LASTEXITCODE -ne 0) { throw 'gen-manifest failed' }

    & $cargo build --release -p saule-engine-lib --manifest-path (Join-Path $repo 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'build failed' }
}

$dll = Join-Path $release 'saule_engine_lib.dll'
$manifest = Join-Path $release 'engine.toml'
foreach ($f in @($dll, $manifest)) {
    if (-not (Test-Path $f)) { throw "$f not found - build saule-engine-lib (release) first" }
}

$sauleHome = if ($env:SAULE_HOME) { $env:SAULE_HOME } else { Join-Path $env:USERPROFILE '.saule' }
$packages = Join-Path $sauleHome 'native_packages'
$manifests = Join-Path $sauleHome 'native_manifests'
New-Item -ItemType Directory -Force -Path $packages, $manifests | Out-Null

Copy-Item $dll (Join-Path $packages 'saule_engine_lib.dll') -Force
Copy-Item $manifest (Join-Path $manifests 'engine.toml') -Force

Write-Host 'installed:'
Write-Host "  $(Join-Path $packages 'saule_engine_lib.dll')"
Write-Host "  $(Join-Path $manifests 'engine.toml')"
