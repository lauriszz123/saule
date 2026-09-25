# Install the `engine` native package into Windows' %USERPROFILE%\.saule.
#
# The package is one file: the library carries its own description (every
# class, signature and doc comment is compiled into it by `saule-sdk`), so
# installing is copying `saule_engine_lib.dll` into `native_packages\`.
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
if (-not (Test-Path $dll)) {
    throw "$dll not found - run 'cargo build --release -p saule-engine-lib' first"
}

$sauleHome = if ($env:SAULE_HOME) { $env:SAULE_HOME } else { Join-Path $env:USERPROFILE '.saule' }
$packages = Join-Path $sauleHome 'native_packages'
New-Item -ItemType Directory -Force -Path $packages | Out-Null

# An install from before packages carried their own description left a
# separate manifest beside the library. It is not used any more.
$manifests = Join-Path $sauleHome 'native_manifests'
Remove-Item (Join-Path $manifests 'engine.toml') -ErrorAction SilentlyContinue
if ((Test-Path $manifests) -and -not (Get-ChildItem $manifests)) {
    Remove-Item $manifests
}

Copy-Item $dll (Join-Path $packages 'saule_engine_lib.dll') -Force

Write-Host "installed: $(Join-Path $packages 'saule_engine_lib.dll')"
