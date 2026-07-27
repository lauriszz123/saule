# Install the `engine` native package into Windows' %USERPROFILE%\.saule.
#
# Unlike the Unix scripts, Cargo already names the Windows cdylib
# `saule_engine_lib.dll` — matching the manifest's `binary` list — so nothing
# is renamed on copy. The manifest is written straight to its install location
# by the `gen-manifest` binary, which renders it from the crate's
# `#[saule_export]` declarations and is built alongside the library.
#
# Usage:
#   pwsh -File scripts\install_windows.ps1            # build, then install
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
}

$dll = Join-Path $release 'saule_engine_lib.dll'
$genManifest = Join-Path $release 'gen-manifest.exe'
foreach ($f in @($dll, $genManifest)) {
    if (-not (Test-Path $f)) {
        throw "$f not found - run 'cargo build --release -p saule-engine-lib' first"
    }
}

$sauleHome = if ($env:SAULE_HOME) { $env:SAULE_HOME } else { Join-Path $env:USERPROFILE '.saule' }
$packages = Join-Path $sauleHome 'native_packages'
$manifests = Join-Path $sauleHome 'native_manifests'
New-Item -ItemType Directory -Force -Path $packages, $manifests | Out-Null

Copy-Item $dll (Join-Path $packages 'saule_engine_lib.dll') -Force

& $genManifest (Join-Path $manifests 'engine.toml')
if ($LASTEXITCODE -ne 0) { throw 'gen-manifest failed' }

Write-Host 'installed:'
Write-Host "  $(Join-Path $packages 'saule_engine_lib.dll')"
Write-Host "  $(Join-Path $manifests 'engine.toml')"
