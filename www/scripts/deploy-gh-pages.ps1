# Build the site locally and publish it to the `gh-pages` branch.
#
# The Windows counterpart of deploy-gh-pages.sh — same steps, same result,
# same `gh-pages` branch. Use whichever matches the shell you are in.
#
# This is the fallback for when GitHub Actions cannot run — an account billing
# lock, a self-hosted setup, or simply wanting to ship without CI. It produces
# exactly the same site the workflow would; the only difference is that your
# machine does the building.
#
# Usage:
#   pwsh -File www\scripts\deploy-gh-pages.ps1           # build, commit, push
#   pwsh -File www\scripts\deploy-gh-pages.ps1 -DryRun   # build and stage only
#
# Prerequisites beyond Node and Git:
#   `bash` must be on PATH. `npm run build` fires the `prebuild` hook, which
#   runs `bash scripts/build-wasm.sh` to compile the playground's WebAssembly.
#   Git for Windows ships a suitable bash; if `bash --version` works in this
#   shell, so will the build. The wasm build itself also needs the
#   `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`)
#   and `wasm-bindgen-cli`.
#
# One-time setup after the first successful run:
#   Settings > Pages > Build and deployment > Source > "Deploy from a branch"
#   Branch: gh-pages / (root)
#
# Note this is the *other* Pages mode from the workflow in
# .github/workflows/deploy-www.yml — pick one. If you later fix billing and go
# back to Actions, switch the Source back to "GitHub Actions"; leaving it on
# the branch means pushes to main stop updating the site.
[CmdletBinding()]
param(
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

$WwwDir = Split-Path -Parent $PSScriptRoot
$RepoRoot = Split-Path -Parent $WwwDir
$Worktree = Join-Path $RepoRoot '.gh-pages-worktree'
$Branch = 'gh-pages'

# Bash aborts on any non-zero exit thanks to `set -e`. PowerShell does not do
# that for native executables — it only throws on *cmdlet* errors — so every
# git/npm call has to be checked by hand or a failed build would sail on and
# publish a stale or half-written site.
function Invoke-Native {
    param(
        [Parameter(Mandatory)][string]$What,
        [Parameter(Mandatory)][string]$Exe,
        [string[]]$Arguments = @()
    )
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$What failed (exit $LASTEXITCODE)"
    }
}

# Run a command whose failure is information rather than an error — a missing
# remote branch, a worktree that isn't there. Returns the exit code.
#
# `2>$null` is what keeps git's `fatal: ...` off the console for the cases we
# are deliberately testing for. In Windows PowerShell 5.1 that redirection
# still turns each stderr line into an ErrorRecord, which is why the whole
# call runs under `Continue` — with the script's usual `Stop` in force, a
# probe for something that is *supposed* to be missing would throw.
function Invoke-Probe {
    param(
        [Parameter(Mandatory)][string]$Exe,
        [string[]]$Arguments = @()
    )
    $previous = $ErrorActionPreference
    $errorMark = $Error.Count
    $ErrorActionPreference = 'Continue'
    try {
        & $Exe @Arguments 2>$null | Out-Null
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
        # Drop the ErrorRecords the redirect manufactured so a later, real
        # failure isn't read against a pile of expected noise.
        while ($Error.Count -gt $errorMark) { $Error.RemoveAt(0) }
    }
}

foreach ($tool in 'git', 'npm') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "$tool is not on PATH. See the prerequisites at the top of this script."
    }
}

# `npm run build` shells out to `bash scripts/build-wasm.sh`, and npm resolves
# `bash` through PATH. Git for Windows installs one but only puts `git` on
# PATH, so the usual machine has a perfectly good bash that npm cannot see.
# Find it next to git and prepend it for this process only — inheriting into
# the npm child is the whole point, and the user's PATH is left alone.
if (-not (Get-Command bash -ErrorAction SilentlyContinue)) {
    $gitExe = (Get-Command git).Source
    $gitRoot = Split-Path -Parent (Split-Path -Parent $gitExe)
    $bashDir = Join-Path $gitRoot 'bin'
    if (Test-Path (Join-Path $bashDir 'bash.exe')) {
        Write-Host "==> Using bash from $bashDir"
        $env:PATH = "$bashDir;$env:PATH"
    } else {
        throw 'bash is not on PATH and none was found alongside git. See the prerequisites at the top of this script.'
    }
}

Set-Location $RepoRoot

if (git status --porcelain) {
    Write-Warning 'The working tree has uncommitted changes.'
    Write-Warning 'The site is built from your files on disk, not from HEAD,'
    Write-Warning 'so those changes will be published.'
    Write-Host ''
}

$Sha = (git rev-parse --short HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'not a git repository' }

Write-Host '==> Building the site'
Set-Location $WwwDir
Invoke-Native -What 'npm run sync-docs' -Exe 'npm' -Arguments @('run', 'sync-docs')
# `npm run build` triggers `prebuild`, which compiles crates/saule-wasm and
# runs wasm-bindgen — the playground's runtime is produced here, not committed.
Invoke-Native -What 'npm run build' -Exe 'npm' -Arguments @('run', 'build')

$Dist = Join-Path $WwwDir 'dist'
if (-not (Test-Path (Join-Path $Dist 'index.html'))) {
    throw 'build produced no dist/index.html'
}

Write-Host "==> Preparing the $Branch worktree"
Set-Location $RepoRoot

# A stale worktree from an interrupted run would block `worktree add`.
Invoke-Probe -Exe 'git' -Arguments @('worktree', 'remove', '--force', $Worktree) | Out-Null
if (Test-Path $Worktree) {
    Remove-Item -LiteralPath $Worktree -Recurse -Force
}

# Track the remote branch if it exists; otherwise start the branch here.
if ((Invoke-Probe -Exe 'git' -Arguments @('ls-remote', '--exit-code', '--heads', 'origin', $Branch)) -eq 0) {
    Invoke-Native -What "git fetch $Branch" -Exe 'git' -Arguments @('fetch', 'origin', $Branch)
    Invoke-Native -What 'git worktree add' -Exe 'git' `
        -Arguments @('worktree', 'add', '-B', $Branch, $Worktree, "origin/$Branch")
} else {
    Write-Host "    (no remote $Branch yet - creating it)"
    Invoke-Native -What 'git worktree add' -Exe 'git' `
        -Arguments @('worktree', 'add', '-B', $Branch, $Worktree)
}

Write-Host '==> Copying the build'
# Clear everything except .git, so files deleted from the site disappear from
# the branch too rather than lingering forever. `-Force` is what makes
# Get-ChildItem return dotfiles; without it a previous deploy's .nojekyll would
# survive every future clear. In a linked worktree `.git` is a *file*, not a
# directory, which the name test handles either way.
Get-ChildItem -LiteralPath $Worktree -Force |
    Where-Object { $_.Name -ne '.git' } |
    Remove-Item -Recurse -Force -Confirm:$false

# Copy the contents of dist/, not dist/ itself. Enumerating with -Force keeps
# any dotfiles Astro emitted; `Copy-Item dist\*` would skip them.
Get-ChildItem -LiteralPath $Dist -Force | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $Worktree -Recurse -Force
}

# Branch-based Pages runs the published files through Jekyll, which silently
# drops every directory whose name starts with an underscore. Astro puts all
# of its CSS and JS in `_astro/`, so without this file the site loads as
# unstyled HTML with no working scripts. The Actions deployment path does no
# Jekyll processing and does not need it — which is why `.nojekyll` is created
# here rather than committed under www/public/.
$NoJekyll = Join-Path $Worktree '.nojekyll'
if (-not (Test-Path -LiteralPath $NoJekyll)) {
    New-Item -ItemType File -Path $NoJekyll | Out-Null
}

Set-Location $Worktree
Invoke-Native -What 'git add' -Exe 'git' -Arguments @('add', '--all')

if ((Invoke-Probe -Exe 'git' -Arguments @('diff', '--cached', '--quiet')) -eq 0) {
    Write-Host '==> No changes to publish; the branch already matches this build.'
    Set-Location $RepoRoot
    Invoke-Native -What 'git worktree remove' -Exe 'git' `
        -Arguments @('worktree', 'remove', '--force', $Worktree)
    exit 0
}

Invoke-Native -What 'git commit' -Exe 'git' -Arguments @('commit', '-m', "Deploy website from $Sha")

if ($DryRun) {
    Write-Host ''
    Write-Host "==> -DryRun: committed to $Branch but not pushed."
    Write-Host "    Inspect it:  git -C $Worktree show --stat"
    Write-Host "    Then push:   git -C $Worktree push origin $Branch"
    Write-Host "    Clean up:    git worktree remove --force $Worktree"
    exit 0
}

Write-Host "==> Pushing $Branch"
Invoke-Native -What 'git push' -Exe 'git' -Arguments @('push', 'origin', $Branch)

Set-Location $RepoRoot
Invoke-Native -What 'git worktree remove' -Exe 'git' `
    -Arguments @('worktree', 'remove', '--force', $Worktree)

Write-Host ''
Write-Host 'Published. If this is the first deploy, set:'
Write-Host "  Settings > Pages > Source > Deploy from a branch > $Branch / (root)"
Write-Host 'Then the site appears at https://lauriszz123.github.io/saule/'
