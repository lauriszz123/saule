# Saule — Public Release & Package Distribution Plan

Rewritten against `229bac6` as an ordered set of steps. Step 0 is **done**;
everything from step 1 on is not.

**The goal.** A user on macOS, Linux, or Windows runs one command, gets the
Saule toolchain and working editor support, and can then
`saule add lauriszz123/uikit` to pull in a package somebody published with a
single `saule publish`.

**No central index, no registry server, no PR to a curated list.** A package is
a public git repo with a `saule.config`. The CLI is the whole system: it creates
projects, installs and removes dependencies, repairs broken installs, and
publishes.

Each step below states what it delivers, what to do, and how you know it's
finished. Steps 1–4 get a public, installable language; 5–9 get the package
manager. Design detail that would clutter a step lives in the appendices.

---

## Where things actually stand

| Area | State today |
|---|---|
| Versioning | **Done.** `26.<build>`, generated on release, readable from the CLI, the LSP, and the language. See step 0. |
| CI | `ci.yml` added in step 0 (fmt, clippy, tests, `.sau` fixtures, version agreement). The two website workflows predate it. |
| Releases | **`release.yml` exists but has never run.** No tags, no published artifacts. |
| Toolchain install | [scripts/install_path.sh](scripts/install_path.sh) symlinks `target/release/saule` — requires a clone plus a Rust toolchain. Unix only. |
| Package manager | **Does not exist.** `saule` has three subcommands: `run`, `fmt`, `init`. |
| Dependencies | Local relative paths only (`dependencies: ["../json"]`). No versions, no remote fetch, no lockfile. |
| Native packages | Work, but install via per-platform shell scripts needing a full Rust build. |
| Editor plugins | All three exist and all three fall back to `saule-lsp` on `$PATH`. **None are published** anywhere. |
| Licensing | **No LICENSE file**, no `license` in any `Cargo.toml` — but `vscode/package.json` claims MIT. |

Four things are already right and shape everything below:

- **`SAULE_HOME` is a well-defined install root**
  ([dynamic_packages.rs:100](crates/saule-interpreter/src/dynamic_packages.rs:100))
  — the natural home for packages, binaries, and docs.
- **Dependencies are resolved in the CLI, not the interpreter.**
  [project.rs:resolve_dependencies](crates/saule-cli/src/project.rs:140) turns
  each `dependencies:` entry into a `Dependency { name, root, src_dirs }` and
  hands it to the interpreter. The interpreter never learns where a dependency
  came from.
- **A dependency's import name comes from its own `saule.config`**
  ([project.rs:170](crates/saule-cli/src/project.rs:170)), not from its path.
- **`clap` has landed.** The surface lives in
  [cli.rs](crates/saule-cli/src/cli.rs) and
  [main.rs](crates/saule-cli/src/main.rs) is dispatch only, so adding eleven
  subcommands with flags is now routine.

The middle two are the key lever: **installed packages need no interpreter
changes at all.** `saule add` puts source in `SAULE_HOME` and `project.rs`
resolves it into the same `Dependency` a relative path produces today.
[module.rs](crates/saule-interpreter/src/module.rs) is untouched.

The third point is what makes an index unnecessary. *Where* a package lives
(`github.com/lauriszz123/uikit`) is decoupled from *what you type to import it*
(`uikit`). A global name registry exists to bind those two together; Saule
doesn't need one because the package declares its own import name.

---

## Step 0 — Versioning — **DONE**

### The scheme

```
26.7
^^ ^
|  └── build number, counting up from 1 within the year
└───── two-digit year
```

Two components, both meaningful, and **no patch component** — a patch would
require someone to decide whether a change is "minor" or "patch", a judgement
that costs real time and tells the person installing the toolchain almost
nothing. `26.7` came after `26.6`, and that is the whole story.

Build numbers **restart each year** and versions still compare correctly,
because the year leads: `27.1` > `26.412` under ordinary numeric comparison.

### It generates itself

Nobody types a version number. `scripts/next-version.sh` reads the existing
`v26.*` tags and prints the next build number; the release workflow tags with
it and passes it to the build as `$SAULE_VERSION`.

[crates/saule-version](crates/saule-version)'s build script resolves the value
at compile time, in this order:

| Source | Result |
|---|---|
| `$SAULE_VERSION` | Used verbatim. This is the release path. |
| A clean tree on a `v26.<n>` tag | `26.<n>`, marked as a release. A locally built tag is byte-identical in version terms to the CI artifact. |
| Any other git state | `26.<highest+1>-dev+<sha>` — the number it is *heading toward*, so code can be written against an unreleased feature and declare `min_saule_version` for it. |
| No git at all | `26.0-dev`. Build 0 is never carried by a release, so "unknown provenance" can't be mistaken for one. |

The **year of record** is the major component of `version` in the workspace
[Cargo.toml](Cargo.toml). That is the only place the year is written down, and
in January it is the only line that changes. Cargo needs three semver
components; its copy is internal metadata that nothing user-facing prints.

### Where the version is readable

| Surface | Form | Value |
|---|---|---|
| `saule --version` | long | `saule 26.7` / `saule 26.8-dev+1a2b3c4` |
| `saule-lsp --version` | long | same, and it no longer hangs — see below |
| LSP `initialize` handshake | long | `server_info.version` |
| `saule.config` `min_saule_version` | short | compared via `saule_version::at_least` |
| `saule init` scaffold | short | never writes a `-dev` version into a new project |
| Saule code | both | `Saule.version`, `Saule.full`, `Saule.year`, `Saule.build`, `Saule.isDev`, `Saule.commit`, `Saule.atLeast(v)` |
| Playground (wasm) | long | `version()` export |
| Editor plugin manifests | `26.7` / `26.7.0` | written by `scripts/stamp-version.sh` |

`Saule.*` lives in
[stdlib/version.rs](crates/saule-interpreter/src/stdlib/version.rs), documented
in [DOCS.md](DOCS.md#saule). It is auto-prelude'd, so no import is needed, and
its members are registered with the typechecker — `Saule.verzion` is a compile
error, and `Saule.atLeast` is known to return `boolean`.

**One comparator, three callers.** `min_saule_version`, `Saule.atLeast`, and
the release tooling all call `saule_version::version_at_least`. A project that
runs can't be one the language disagrees about.

### What this step also fixed

- **`saule-lsp --version` used to hang.** The binary parsed no arguments, so it
  started the server and blocked on stdin forever — and both `install_path.sh`
  and the docs tell users to run exactly that as their check that the language
  server installed. It now answers `--version` and `--help` before touching
  stdin, and rejects unknown flags with exit 2 rather than silently serving.
- **Every `min_saule_version` had to be migrated.** The old values said
  `2026.1.0`; under numeric comparison `2026` is a *higher* year than `26`, so
  all seven example projects would have refused to run. Migrated to `26.1`,
  with a regression test pinning that the old spelling does not silently pass.
- **`run_tests.sh` always exited 0**, even with failing fixtures, which would
  have made the new CI job decorative. It now exits non-zero and prints a
  count.

### Cutting a release

```bash
gh workflow run release.yml
```

That's it: the workflow picks the number, creates the tag, builds all six
triples, verifies each binary reports the version the tag claims, and publishes
the archives with a `SHA256SUMS`. `--field dry_run=true` builds without tagging
or publishing. Pushing a `v26.7` tag by hand is the escape hatch for re-cutting
a release whose build failed for an infrastructure reason.

Before publishing the editor plugins, run `scripts/stamp-version.sh <version>`
and commit — their manifests are read by marketplaces long before any Rust runs.
`scripts/stamp-version.sh <version> --check` verifies without writing.

---

## Step 1 — License — **DONE**

**MIT**, copyright "Saule contributors". This was the blocker for everything
else: without an explicit license the code was "all rights reserved" by
default, which flatly contradicted the MIT claim already published in
[vscode/package.json](editors/vscode/package.json). A release archive with no
license is also not redistributable — `release.yml` warns when it packages
without one.

- [LICENSE](LICENSE) at the repo root.
- `license = "MIT"` in `[workspace.package]`, `license.workspace = true` in all
  16 crates.

**Still open:** confirm the GitHub repo is public before anything points users
at it. Not verifiable from this machine — `gh` is not installed.

---

## Step 2 — Prove the release pipeline

### Blocked: the GitHub account is locked for billing

**Nothing in CI can run until this is fixed.** Every job on the repository
fails before its first step with:

```
The job was not started because your account is locked due to a billing issue.
```

This is not specific to the new workflows. `Check website` and `Deploy website`
have failed on **every run since they were added on 2026-07-29** — they have
never once succeeded. The site is live only because
`www/scripts/deploy-gh-pages.sh` pushes the built output to the `gh-pages`
branch by hand, and GitHub's own managed "pages build and deployment" job is
billed differently, so it still runs.

The repository is public, so Actions minutes are free — this is an
account-level lock (payment method, spending limit, or an unpaid invoice), not
a minutes overage. Fix it under **GitHub → Settings → Billing**. That is
account and payment work, so it has to be done by hand; it is not something
this plan can automate.

`v26.1` **is already tagged and pushed**, and the build never started, so no
release was published and nothing is half-done. Once billing is unlocked, just
re-run the failed `Release` run — the tag-push path reads the version from the
existing tag, so the number does not need to be burned or re-cut.

### Then, once it runs

`release.yml` and `ci.yml` have still never executed a single step. Everything
downstream assumes they work.

**Do:**

1. Push, and confirm `ci.yml` goes green.
2. **Clear the pre-existing lint drift, then make the lint steps blocking.**
   `cargo fmt --all --check` reports roughly 190 hunks and clippy reports a
   handful of warnings, none of it from step 0's work — it predates the
   existence of any CI to catch it. Run `cargo fmt --all`, clear the clippy
   warnings, commit that on its own, then delete the two `continue-on-error:
   true` lines in `ci.yml`. Until that happens the two steps report drift
   without failing the build, because a CI that is red on arrival is a CI
   everyone learns to ignore.
3. `gh workflow run release.yml --field dry_run=true`. Confirm all six triples
   build and each one's version self-check passes.
4. Fix whatever the first real run surfaces. The two known unknowns:
   - The `actions/upload-artifact` / `download-artifact` major versions are set
     to match the era of the actions already used by `deploy-www.yml`. A wrong
     major fails immediately and obviously.
   - `ubuntu-24.04-arm` runners are free for public repositories; if the repo
     is private this entry needs `cross` instead.
5. Consider adding `[profile.release]` with `lto = "thin"` and
   `strip = "symbols"` before the first real release — these binaries embed a
   whole interpreter and the workspace sets no release profile today.
6. `gh workflow run release.yml` for real. That publishes `v26.1`.

**Done when** a GitHub Release exists with six archives and a `SHA256SUMS`.

---

## Step 3 — The one-line installer

The headline deliverable: on
[lauriszz123.github.io/saule](https://lauriszz123.github.io/saule/),

```bash
curl -fsSL https://lauriszz123.github.io/saule/install.sh | sh
```

```powershell
irm https://lauriszz123.github.io/saule/install.ps1 | iex
```

**Serve the scripts from the site, not from `raw.githubusercontent.com`.** Put
them in `www/public/install.sh` and `www/public/install.ps1`. Astro copies
`public/` verbatim into `dist/`, which deploys to `/saule/`, so they land at
exactly those URLs — and `deploy-www.yml`'s existing `www/**` path filter
already redeploys them. No new workflow, and no second copy of the script to
drift.

### `install.sh`, in order

POSIX `sh` throughout — no bash, no `jq`.

1. **Detect the triple** from `uname -s` / `uname -m`. On Linux, select `musl`
   when `ldd --version` doesn't mention glibc; otherwise a musl-only box gets a
   binary that cannot start.
2. **Resolve the version.** Honour `$SAULE_VERSION`, else `curl -sI` the
   `releases/latest` URL and parse the redirect `Location` for the tag. Not the
   GitHub API — its unauthenticated rate limit is per-IP, which is how a whole
   office behind one NAT discovers the installer is broken.
3. **Download** the archive and `SHA256SUMS` into `mktemp -d`, with a `trap` to
   clean up.
4. **Verify** with `sha256sum -c` / `shasum -a 256 -c`, filtered to our
   filename. Abort loudly if neither tool exists — never skip silently. This is
   not optional: `curl | sh` installing an unverified binary is the single
   riskiest thing in this plan.
5. **Install atomically.** Unpack into `$SAULE_HOME/tmp/<pid>/`, then `mv` the
   two binaries into `$SAULE_HOME/bin/`. Same filesystem, so it is a rename —
   an interrupted install cannot leave a half-written `saule`. Create `bin/`,
   `packages/`, `cache/`, `tmp/` up front.
6. **Write `$SAULE_HOME/env`** — a sourceable snippet that prepends `bin` to
   `PATH` only when absent — then append `. "$HOME/.saule/env"` to each of
   `~/.zshrc`, `~/.bashrc`, `~/.profile` that exists, guarded by `grep -F` so
   re-running is idempotent. Fish needs its own `fish_add_path` line. This
   replaces `install_path.sh`'s three-file `export PATH` duplication.
7. **Verify and print.** Run `$SAULE_HOME/bin/saule --version` — which now
   works for `saule-lsp` too — and tell the user to `exec $SHELL -l`.

`$SAULE_HOME` must be honoured verbatim when set, matching
[dynamic_packages.rs:100](crates/saule-interpreter/src/dynamic_packages.rs:100):
it *is* the directory, not a parent to append `.saule` to.

`install.ps1` mirrors this with `%USERPROFILE%\.saule\bin`,
`Get-FileHash -Algorithm SHA256`, `Expand-Archive`, and
`[Environment]::SetEnvironmentVariable('Path', …, 'User')`.

### macOS signing: less than previously assumed

An earlier draft of this plan budgeted for Apple notarization. For this
delivery path that is not needed:

- Quarantine is applied by the *downloading application*. `curl` doesn't set
  `com.apple.quarantine`, so a `curl | sh` install is not Gatekeeper-gated.
- Cargo ad-hoc-signs arm64 macOS binaries at link time, which is all the
  arm64 kernel requires in order to execute them.

So: ship unsigned, and document `xattr -dr com.apple.quarantine ~/.saule/bin`
for people who download the tarball in a browser instead. Revisit the $99/yr
Developer account only if a `.pkg` or `.dmg` is ever wanted.

### Rewrite the install page

[installation.md](www/src/content/docs/guides/installation.md) currently says
Saule "installs from source" and requires a Rust toolchain. It becomes: tabbed
one-liners at the top, build-from-source demoted to a contributor section. It's
hand-written, not generated by `sync-docs`, so it's a straight edit.

The existing `install_mac.sh` / `install_wsl.sh` / `install_windows.ps1` are
**developer** scripts for working on engine-lib locally, and should be
documented as such rather than as the user-facing path.

**Done when** a fresh machine on each of the three platforms goes from nothing
to a working `saule --version` with one command.

---

## Step 4 — Editor plugins

All three plugins already fall back to `saule-lsp` on `$PATH`
([SauleToolchain.kt:74](editors/intellij/src/main/kotlin/com/saule/lang/SauleToolchain.kt),
[extension.ts:178](editors/vscode/src/extension.ts)), so **once step 3 puts the
server on PATH, all three work as-is.** The gap is publication, not function.

**Neovim** — one code change needed first.
[lsp.lua:27](editors/nvim/lua/saule/lsp.lua) resolves the server by walking up
from its own file to a repo checkout. Change the default to plain `saule-lsp`,
keeping repo detection as an opt-in for contributors. Then it installs cleanly
via lazy.nvim or packer pointing at the repo. Longer term, upstream the
filetype and LSP config into `nvim-lspconfig`.

**VS Code** — needs a real Marketplace publisher ID (`package.json` says
`"publisher": "saule"`, which must be claimed or changed), a PAT in CI, and
`vsce publish` on tag. Worth publishing to OpenVSX for VSCodium users too.

**IntelliJ** — needs a JetBrains Marketplace account and token plus the
`publishPlugin` Gradle task. One open question: the platform wants
`sourceCompatibility=21` while
[gradle.properties](editors/intellij/gradle.properties) pins `javaVersion=17`.
The LSP4IJ runtime dependency is already declared correctly, and the
previously-noted non-executable `gradlew` is already fixed (mode `100755`).

Run `scripts/stamp-version.sh <version>` before packaging either plugin.

**Done when** all three install from their normal channel and give diagnostics
in a project outside this repo.

---

## Step 5 — `saule check`, and the config/lock format

The base every package-manager command builds on.

**`saule check`** — lex, parse, and typecheck the whole project without
executing it. An early exit from the pipeline already in
[run.rs](crates/saule-cli/src/run.rs). Needed by `saule add` (to validate a
fetched package) and by `saule publish` (as a preflight), so it comes first.

**The config additions** — see [Appendix A](#appendix-a--package-format).

**The lockfile** — see [Appendix B](#appendix-b--the-lockfile).

**Done when** `saule check` passes on every project in `examples/` and the
parser round-trips a `saule.lock`.

---

## Step 6 — Consuming packages

`saule add` / `install` / `remove` / `list`. **Direct dependencies only** — no
transitive resolution — if that ships it sooner. Because versions are exact
pins and the resolution rule is "highest minimum wins", adding transitive
resolution in step 8 changes no format and invalidates no lockfile written
before it.

```bash
saule add lauriszz123/uikit
```

```
  Resolving lauriszz123/uikit … 1.2.0 (tag v1.2.0, 8f3a1c2)
  Fetching  … 142 KB
  Verifying … sha256 ok
  Checking  … typechecks against Saule 26.7
  Installed uikit 1.2.0

  import uikit
```

That last line is deliberate: the command ends by telling you the exact thing
to type next.

Atomicity, fetching, and the `SAULE_HOME` layout are in
[Appendix C](#appendix-c--resolution-atomicity-and-repair) and
[Appendix D](#appendix-d--saule_home-layout).

**Done when** a fresh clone of a project with a `saule.lock` runs after one
`saule install`.

---

## Step 7 — Publishing and repair

`saule publish`, `saule init --lib` (mostly done — see below), `saule doctor`,
`saule clean`.

`saule init --lib` **already exists** and scaffolds `kind: "library"`,
`src/init.sau`, a `.gitignore`, and a README with the dependency line
pre-filled ([init.rs](crates/saule-cli/src/init.rs)). Only the LICENSE prompt
is missing.

`saule publish` is a preflight-then-commit sequence — every check that can fail
fails **before** anything is pushed, because a bad tag is far more annoying to
undo than a rejected publish. Full sequence in
[Appendix E](#appendix-e--saule-publish).

**Third-party packages use semver, not the toolchain's scheme.** The toolchain
is `26.7` because it is one artifact with one changelog. A library needs to
communicate compatibility to consumers, which is what a major version is for,
so `saule publish` should require a semver `version:` and reject anything else.
Two different schemes in one ecosystem is a real cost, but the alternative is
either denying libraries a compatibility signal or giving the toolchain a patch
component that means nothing. Worth settling before the first package exists.

`min_saule_version` compares cleanly against either scheme —
`saule_version::version_at_least` is a plain numeric dotted compare.

**Done when** you can publish a package you wrote and install it on another
machine, and `saule doctor --fix` recovers from a deleted `packages/` entry.

---

## Step 8 — Real dependency graphs

`saule update`, transitive resolution, `--as`. See
[Appendix C](#appendix-c--resolution-atomicity-and-repair).

---

## Step 9 — Native release assets

`saule add <native-package>` with no Rust toolchain required. See
[Appendix F](#appendix-f--native-packages).

---

## Also worth doing early

**UIKit does not exist yet.** It's the motivating package for the whole
source-package path and the first real test of `saule publish`. Ship it as a
stub during step 6 — publishing a package you wrote is the only way to find out
whether the flow is actually seamless.

**`examples/json_usage` currently fails to typecheck** (`Json.decode` returns
`any?` assigned to a `table?`). Pre-existing, unrelated to versioning, but
`ci.yml` runs the fixtures now, so it will show up as a red build.

---

# Appendices

## Appendix A — Package format

Nothing makes a repo a Saule package but a `saule.config` at the root and a git
tag. That is the whole contract, and it's what `saule init --lib` scaffolds.

```
uikit/
├── saule.config
├── src/
├── LICENSE
└── README.md
```

`saule.config` gains a handful of optional keys. The format is unchanged —
still flat `key: "value"` lines, `--` comments, list values, parsed by
[read_config](crates/saule-cli/src/project.rs:97):

```
name: "uikit"                       -- the import name; what users type
version: "1.2.0"                    -- must match the git tag v1.2.0
repo: "https://github.com/lauriszz123/uikit"
kind: "library"                     -- or "app" (default); libraries need no entry
description: "Terminal UI widgets"
license: "MIT"
src_dirs: ["src"]
min_saule_version: "26.1"

dependencies: [
  "../json",                        -- local path (unchanged, still works)
  "lauriszz123/uikit@1.2.0",        -- GitHub shorthand
  "gitlab.com/team/thing@0.4.0",    -- any git host
  "codeberg.org/x/y@main",          -- a branch: a deliberate escape hatch
]
```

**Only `name` and `version` are required to publish.** `repo` is inferred from
`git remote origin` when absent — one less thing the author has to get right.

### Dependency entry grammar

Resolution order per entry, first match wins:

| Shape | Meaning |
|---|---|
| starts with `.`, `/`, `~` | local path — today's behaviour, bit-for-bit |
| `<host>/<owner>/<repo>[@req]` where host contains a `.` | git package on that host |
| `<owner>/<repo>[@req]` | git package, host defaults to `github.com` |

### No version ranges

`@ver` is **a single exact version** — `1.2.0` or `v1.2.0`, both accepted as
tag spellings — or omitted, meaning the latest stable tag at the time you ran
`saule add`. There is no `^`, no `~`, no `>=`.

Ranges look load-bearing and aren't, because `saule.lock` already pins a SHA. A
project with `uikit@^1.2` and a lock installs exactly what the lock says; it
does *not* pick up 1.2.1 on the next build. A range only takes effect when you
re-resolve, and re-resolving is `saule update` — a command the user ran on
purpose, producing a reviewable diff. **The caret is a range operator smuggled
into a data file; `saule update` is the same operator as a command, and the
command is the better spelling.**

The one thing ranges genuinely buy is transitive deduplication, and Appendix C
solves that without them: the version in a config is a **minimum**, and the
resolver installs the highest version any dependency asked for. No requirement
grammar, no backtracking, and no "no version satisfies all constraints" errors
— the hardest class of error a package manager produces, and one this design
cannot generate.

What this costs: nobody receives a patch release without running
`saule update`. The lockfile already imposed that, so it isn't a new cost.

A `@` value that parses as neither a version nor a tag — a branch name, a
40-char SHA — resolves as a direct git ref and is recorded in the lock as such.
Useful for testing an unreleased fix, visibly not a version, and skipped by
`saule update`. `saule publish` warns when a package depends on one.

If ranges ever turn out to be necessary, adding them later is backwards
compatible: every exact pin is a valid range. Going the other way is not.

## Appendix B — The lockfile

New `saule.lock`, committed by the user:

```
uikit 1.2.0 https://github.com/lauriszz123/uikit rev=<40-char-sha> sha256=<digest>
json  0.4.0 https://github.com/lauriszz123/saule-json rev=<sha> sha256=<digest>
```

Records the **resolved commit SHA and content digest**, not just the version,
so a force-pushed or moved tag cannot silently change what a build gets. This
matters more without an index, not less: with no curated list, tag immutability
is the only integrity guarantee, and git does not provide it. `saule install`
is reproducible from the lock alone; `saule update` is the only command that
rewrites it.

Same flat, line-oriented format as `saule.config` — one more thing that doesn't
need a TOML parser.

## Appendix C — Resolution, atomicity, and repair

### Fetch

Requirement resolution needs the tag list, which means `git ls-remote --tags`.
**`git` is a hard dependency of `saule add` / `update` / `publish`**, and only
of those — `install.sh` and `saule run` never touch it. That's the honest cost
of dropping the index, and a fair trade: anyone publishing or consuming
git-hosted packages has git.

For GitHub, the CLI takes a fast path: resolve tags via `git ls-remote`, then
download `codeload.github.com/<owner>/<repo>/tar.gz/<sha>` over plain HTTPS
instead of cloning. Same integrity story (the SHA is pinned), a fraction of the
bytes, and no `.git` in `SAULE_HOME`. Other hosts fall back to a shallow
`git clone --depth 1` at the resolved SHA.

### Transitive dependencies

Depth-first over each dependency's own `saule.config`. **One version per import
name per project** — two versions of `uikit` in one build would collide in the
interpreter's module namespace.

With no ranges, the rule that satisfies that constraint is one line:

> A version in a config is a **minimum**. For each package, install the highest
> version any dependency asked for.

Your project pins `uikit 1.2.0`, `toolkit` pins `uikit 1.3.0` → install 1.3.0.
Deterministic, order-independent, computable in a single pass with no
backtracking. Adding a dependency can only ever raise a version, never lower
one, so a new dep cannot silently change an unrelated part of the graph.

The only unsatisfiable case is a **major** disagreement, which is a real
incompatibility rather than a solver failure, and is reported as such:

```
error: incompatible versions of `uikit`
  your project    requires uikit 1.2.0
  toolkit 0.9.0   requires uikit 2.1.0
  1.x and 2.x are not interchangeable. Either:
    saule update uikit                              -- move your project to 2.x
    saule add lauriszz123/uikit --as uikit2         -- keep both, separate names
```

Selected versions land in `saule.lock` with their SHAs, so the resolver runs on
`add` and `update` only — never on `install`, and never on `run`.

Version-scoped directories in `SAULE_HOME` still matter: two *projects* can use
different versions concurrently. Only within a single build is a single version
enforced.

Two unrelated repos both declaring `name: "uikit"` is the other collision case,
caught at install with the same shape of error and fixed by `--as`.

### Atomic install

`saule add` never leaves the project in a half-installed state:

1. Fetch to `~/.saule/tmp/<random>/`.
2. Verify the digest, parse the package's `saule.config`, check
   `min_saule_version`, resolve its transitive deps, and typecheck it.
3. On any failure: delete the temp directory, leave `saule.config` and
   `saule.lock` **untouched**, report what failed and at which step.
4. On success: `rename()` the temp directory into
   `packages/<host>/<owner>/<repo>/<version>/` — atomic on every supported
   platform — then write config and lock together.

Because the package tree is only ever written by rename, an interrupted install
(Ctrl-C, crash, full disk) leaves garbage in `tmp/` and nothing else.
`saule clean` sweeps it.

### Repair

`saule doctor` is the "it errored, get me back to working" command:

- package in the lock but missing from disk → offer to reinstall
- package on disk with a wrong digest → offer to reinstall
- `saule.config` and `saule.lock` disagree → offer to re-resolve
- dependency fails to typecheck against the current toolchain → name it,
  suggest `saule update <pkg>` or `saule remove <pkg>`
- import-name collision → suggest the `--as` invocation, spelled out in full

Every diagnosis prints the exact command that fixes it. `saule doctor --fix`
applies the unambiguous ones.

`saule remove <pkg>` always succeeds, even if the package is broken or missing
on disk — it edits config and lock, and never depends on the thing it's
removing being loadable. A package manager you can't uninstall from when it's
broken is the worst failure mode there is.

## Appendix D — `SAULE_HOME` layout

Additive — the two existing directories keep their names and meaning, so
current installs stay valid.

```
~/.saule/
├── bin/                          saule, saule-lsp                    (step 3)
├── packages/                     source packages                     (step 6)
│   └── github.com/
│       └── lauriszz123/uikit/
│           └── 1.2.0/            unpacked, immutable, read-only
├── native_packages/              compiled cdylibs                    (exists)
├── native_manifests/             TOML manifests                      (exists)
├── cache/                        downloaded tarballs, git metadata   (step 6)
├── tmp/                          staging for atomic installs         (step 3)
├── docs/                         offline docs                        (later)
└── env                           sourceable PATH snippet             (step 3)
```

Packages are keyed by **host + path + version**, not by name. Two different
repos can both call themselves `uikit` and coexist on disk; the collision is
caught at resolution time in one project, not globally at install time. Version
directories are immutable — an install is a fresh unpack, never a mutation.

One thing the LSP needs from this: dependency sources live in `SAULE_HOME`, so
go-to-definition into a package resolves to a read-only file outside the
workspace. The server already receives `src_dirs` per dependency; it should
mark those roots read-only rather than letting a user edit an immutable
installed package.

## Appendix E — `saule publish`

The author's entire workflow, from a repo that already has a `saule.config`:

```bash
saule publish
```

**Preflight** (read-only):

1. Working tree is clean and on a branch with an upstream.
2. `saule.config` has `name` and `version`. Missing `repo`, `description`, or
   `license` are inferred from `git remote origin` / README / LICENSE where
   possible, and reported as warnings otherwise.
3. `name` is a valid import identifier.
4. `saule check` passes. `saule fmt --check` passes (warning, not an error).
5. Every `dependencies:` entry is a published package — **a local relative path
   is a hard error when publishing**, since it cannot resolve on anyone else's
   machine. This is the single most likely first-publish mistake and deserves a
   named error, not a download-time failure for the consumer.
6. Tag `v<version>` does not already exist locally or on the remote. If it
   does: "version 1.2.0 is already published; bump `version` in saule.config".
7. The remote is reachable and **public** — an anonymous probe of the clone URL.
   A private repo publishes "successfully" and then fails for every consumer.

**Commit** (in order, each reversible until the push):

8. Annotated tag `v<version>`.
9. Push the tag.
10. Create the GitHub Release via the API (token from `gh auth token`, or
    `GITHUB_TOKEN`, or `--no-release` to stop after the tag). Attach native
    assets if `native: true`.
11. Print the install line: `saule add lauriszz123/uikit`.

`saule publish <name>` sets `name:` in `saule.config` when it's absent, so a
repo that was never set up as a package can be published in one command.
`upload` is an alias, but `publish` is the primary spelling — "upload" implies a
server that doesn't exist here.

**Yanking**, without an index: delete the GitHub Release and the tag. Consumers
pinned by SHA in their lock are unaffected — that is the intended behaviour, and
worth documenting. `saule update` then simply stops seeing the version.

## Appendix F — Native packages

A native package is the same thing: a public repo whose `saule.config` says
`native: true`. Its **GitHub Release assets** are the distribution, replacing
what the index would otherwise record:

```
uikit-engine-1.2.0-aarch64-apple-darwin.dylib
uikit-engine-1.2.0-x86_64-unknown-linux-gnu.so
uikit-engine-1.2.0-x86_64-pc-windows-msvc.dll
uikit-engine-1.2.0-SHA256SUMS
engine.toml
```

`saule add` picks the asset matching the host triple, verifies it against the
`SHA256SUMS` asset, and drops it into `native_packages/` + `native_manifests/`
— the exact layout the install scripts produce today, so
[dynamic_packages.rs](crates/saule-interpreter/src/dynamic_packages.rs)
discovery works unchanged.

If no asset matches the host triple: fall back to building from source **only
when a Rust toolchain is present and the user passes `--build`**, otherwise fail
with the missing triple named. Silent fallback to a source build is a nasty
surprise on a slow machine, and an implicit one is arbitrary code execution at
install time.

`saule publish` on a native package builds the matrix in CI and uploads the
assets as part of step 10 — the author runs the same one command.

Non-GitHub hosts have no release-asset equivalent worth special-casing. Native
packages are GitHub-only at first; source packages work anywhere.

## Appendix G — Decisions worth keeping visible

- **Name collisions are a local problem, not a global one.** Nobody can squat
  `uikit`, but two packages can both claim it. `--as` handles it. The cost is
  that "the uikit package" is ambiguous in conversation and docs, the way it
  isn't on npm. That's the real price of no index, and the right price at this
  scale.
- **Discovery.** With no index there is no `saule search`. A curated
  `awesome-saule` list in the docs is the honest answer for now; a GitHub topic
  (`saule-package`) makes search possible later without committing to
  infrastructure.
- **`git` as a hard dependency of `add`.** If that ever becomes unacceptable, a
  GitHub-API-only path covers most cases — but it wouldn't cover any other host.
- **Tag mutability.** The lock pins SHAs, which handles it for consumers.
  `saule publish` also refuses to overwrite an existing tag, so authors don't do
  it to themselves.
- **No version ranges.** Settled as a decision, not an omission.
- **`saule add` running build scripts.** Native packages ship prebuilt.
  `--build` is the only path to executing package-authored build code, and it
  stays explicit.
- **Offline mode.** `saule install --offline` from cache matters for CI, and is
  cheap once `cache/` exists.
- **Two version schemes.** The toolchain is `26.<build>`; third-party packages
  are semver. Justified in step 7.
