# Saule — Public Release & Package Distribution Plan

Status: **proposal, nothing implemented.** Written against `c1e7b00`.

Goal: a user on macOS, Linux, or Windows runs one command, gets the Saule
toolchain and working editor support, and can then `saule add lauriszz123/uikit`
to pull in a package that somebody published with a single `saule publish`.

**No central index, no registry server, no PR to a curated list.** A package is
a public git repo with a `saule.config`. The CLI is the whole system: it
creates projects, installs and removes dependencies, repairs broken installs,
and publishes.

---

## 1. Where things actually stand

| Area | State today |
|---|---|
| CI / releases | **None.** No `.github/workflows`, no tags published, no prebuilt binaries. |
| Toolchain install | [scripts/install_path.sh](scripts/install_path.sh) symlinks `target/release/saule` — requires a clone + Rust toolchain. Unix only. |
| Package manager | **Does not exist.** `saule` has exactly three subcommands: `run`, `fmt`, `init`. |
| Dependencies | Local relative paths only (`dependencies: ["../json"]`). No versions, no remote fetch, no lockfile. |
| Native packages | Work, but install via per-platform shell scripts needing a full Rust build. |
| Editor plugins | All three exist and all three fall back to `saule-lsp` on `$PATH`. **None are published** anywhere. |
| Licensing | **No LICENSE file**, no `license` in any `Cargo.toml` — but `vscode/package.json` claims MIT. |

Three things are already right and shape the design:

- **`SAULE_HOME` is a well-defined install root** ([dynamic_packages.rs:100](crates/saule-interpreter/src/dynamic_packages.rs:100)) — the natural home for packages, binaries, and docs.
- **Dependencies are resolved in the CLI, not the interpreter.** [project.rs:resolve_dependencies](crates/saule-cli/src/project.rs:140) turns each `dependencies:` entry into a `Dependency { name, root, src_dirs }` and hands it to the interpreter. The interpreter never learns where a dependency came from.
- **A dependency's import name already comes from its own `saule.config`** ([project.rs:170](crates/saule-cli/src/project.rs:170)), not from its path.

Those last two are the key lever. **Installed packages need no interpreter
changes at all** — `saule add` puts source in `SAULE_HOME`, and `project.rs`
resolves it into the same `Dependency` a relative path produces today.
[module.rs](crates/saule-interpreter/src/module.rs) is untouched.

The third point is what makes an index unnecessary: *where* a package lives
(`github.com/lauriszz123/uikit`) is decoupled from *what you type to import it*
(`uikit`). A global name registry exists to bind those two together. Saule
doesn't need one because the package declares its own import name.

---

## 2. `SAULE_HOME` layout

Additive — the two existing directories keep their names and meaning, so
current installs stay valid.

```
~/.saule/
├── bin/                          saule, saule-lsp                    (new)
├── packages/                     source packages                     (new)
│   └── github.com/
│       └── lauriszz123/uikit/
│           └── 1.2.0/            unpacked, immutable, read-only
├── native_packages/              compiled cdylibs                    (exists)
├── native_manifests/             TOML manifests                      (exists)
├── cache/                        downloaded tarballs, git metadata   (new)
├── tmp/                          staging for atomic installs         (new)
├── docs/                         offline docs                        (new)
└── env                           sourceable PATH snippet             (new)
```

Packages are keyed by **host + path + version**, not by name. Two different
repos can both call themselves `uikit` and coexist on disk; the collision is
caught at resolution time in one project, not globally at install time. Version
directories are immutable — an install is a fresh unpack, never a mutation.

---

## 3. What makes a repo a Saule package

Nothing but a `saule.config` at the repo root and a git tag. That is the whole
contract, and it's what `saule init --lib` scaffolds.

```
uikit/
├── saule.config
├── src/
│   └── ...
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
min_saule_version: "2026.1.0"

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
| starts with `.`, `/`, `~` | local path — today's behavior, bit-for-bit |
| `<host>/<owner>/<repo>[@req]` where host contains a `.` | git package on that host |
| `<owner>/<repo>[@req]` | git package, host defaults to `github.com` |

### No version ranges

`@ver` is **a single exact version** — `1.2.0` or `v1.2.0`, both accepted as tag
spellings — or omitted, meaning the latest stable tag at the time you ran
`saule add`. There is no `^`, no `~`, no `>=`.

Ranges look load-bearing and aren't, because `saule.lock` already pins a SHA. A
project with `uikit@^1.2` and a lock installs exactly what the lock says; it
does *not* pick up 1.2.1 on the next build. A range only takes effect when you
re-resolve, and re-resolving is `saule update` — a command the user ran on
purpose, producing a reviewable diff. **The caret is a range operator smuggled
into a data file; `saule update` is the same operator as a command, and the
command is the better spelling.** Explicit, dated by a commit, visible in review.

The one thing ranges genuinely buy is transitive deduplication, and §6 solves
that without them: the version in a config is a **minimum**, and the resolver
installs the highest version any dependency asked for. That's the whole
algorithm. No requirement grammar, no backtracking, and no "no version
satisfies all constraints" errors — the hardest class of error a package
manager produces, and one this design simply cannot generate.

What this costs: nobody receives a patch release without running `saule update`.
The lockfile already imposed that, so it isn't a new cost.

A `@` value that parses as neither a version nor a tag — a branch name, a
40-char SHA — resolves as a direct git ref and is recorded in the lock as such.
Useful for testing an unreleased fix, visibly not a version, and skipped by
`saule update`. `saule publish` warns when a package depends on one.

If ranges ever turn out to be necessary, adding them later is backwards
compatible: every exact pin is a valid range. Going the other way is not.

### Lockfile

New `saule.lock`, committed by the user:

```
uikit 1.2.0 https://github.com/lauriszz123/uikit rev=<40-char-sha> sha256=<digest>
json  0.4.0 https://github.com/lauriszz123/saule-json rev=<sha> sha256=<digest>
```

Records the **resolved commit SHA and content digest**, not just the version, so
a force-pushed or moved tag cannot silently change what a build gets. This
matters more without an index, not less: with no curated list, tag immutability
is the only integrity guarantee, and git does not provide it. `saule install`
is reproducible from the lock alone; `saule update` is the only command that
rewrites it.

Same flat, line-oriented format as `saule.config` — one more thing that doesn't
need a TOML parser.

---

## 4. CLI surface

The CLI is the product here. Everything a package author or consumer does goes
through it; nobody hand-edits `saule.config` dependencies and nobody opens a PR
against an index.

| Command | Behavior |
|---|---|
| `saule run [target] [-- args]` | Project or single file, decided by whether `target` is a directory. **Done** — `clap` migration landed, replacing the old extension-sniffing dispatch. |
| `saule init <name>` | Scaffold an app (exists today). |
| `saule init <name> --lib` | Scaffold a publishable library: no `entry`, `kind: "library"`, LICENSE prompt, `.gitignore`, README with the `saule add` line pre-filled. |
| `saule add <pkg>[@req]` | Resolve → fetch → verify → stage → typecheck → commit. Updates `saule.config` + `saule.lock`. Atomic; see §6. |
| `saule add <pkg> --as <alias>` | Install under a different import name. Resolves name collisions between two unrelated repos. |
| `saule remove <pkg>` | Drop from config and lock. Cache untouched. |
| `saule install` | Install exactly what the lock pins. The fresh-clone / CI command. |
| `saule update [pkg]` | Bump to the latest published version; rewrite config **and** lock. This is the only command that changes a version, and it does so visibly. |
| `saule list [--tree]` | Installed packages, versions, and where each came from. `--tree` shows the transitive graph. |
| `saule check` | Lex + parse + typecheck the whole project, no execution. Early exit from the existing pipeline in [run.rs](crates/saule-cli/src/run.rs). |
| `saule doctor` | Diagnose and offer to repair: broken installs, missing packages, config/lock drift, name collisions, stale toolchain. |
| `saule clean [--all]` | Prune the cache and any `packages/` entry no longer referenced by any known project. |
| `saule publish` | Validate → tag → push → GitHub Release (+ native assets). Aliased as `saule upload`. |
| `saule self update` | Replace the toolchain in place. |

**`clap` is adopted** — the surface lives in [cli.rs](crates/saule-cli/src/cli.rs)
and [main.rs](crates/saule-cli/src/main.rs) is dispatch only. That was a
prerequisite: the old hand-rolled `match` on `args[0]` would not have survived
eleven more commands with flags, and per-subcommand `--help` is a significant
part of "seamless" for a tool with no web docs yet.

It also removed the guesswork from `saule run`, which previously used five
heuristics (arg count, cwd `saule.config`, `.sau` extension, directory check,
`--` position) to decide between project and single-file mode. Now `--` means
only "script arguments", and one directory check picks the mode.

### Consuming a package

```bash
saule add lauriszz123/uikit
```

```
  Resolving lauriszz123/uikit … 1.2.0 (tag v1.2.0, 8f3a1c2)
  Fetching  … 142 KB
  Verifying … sha256 ok
  Checking  … typechecks against Saule 2026.1.0
  Installed uikit 1.2.0

  import uikit
```

That last line is deliberate: the command ends by telling you the exact thing
to type next.

---

## 5. Publishing: `saule publish`

The author's entire workflow, from a repo that already has a `saule.config`:

```bash
saule publish
```

The command is a preflight-then-commit sequence. Every check that can fail
fails **before** anything is pushed, because a bad tag is far more annoying to
undo than a rejected publish.

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
    assets if `native: true` (§7).
11. Print the install line: `saule add lauriszz123/uikit`.

`saule publish <name>` sets `name:` in `saule.config` when it's absent, so a
repo that was never set up as a package can be published in one command. That
is the `saule upload <name>` shape you described; `publish` is the primary
spelling and `upload` an alias, because "upload" implies a server that doesn't
exist here.

**Yanking**, without an index: delete the GitHub Release and the tag. Consumers
pinned by SHA in their lock are unaffected — that is the intended behavior, and
worth documenting. `saule update` then simply stops seeing the version.

---

## 6. Resolution, atomicity, and repair

You asked specifically for installs that back out cleanly when something goes
wrong. That's a design constraint, not an error path bolted on later.

### Fetch

Requirement resolution needs the tag list, which means `git ls-remote --tags`.
**`git` is a hard dependency of `saule add` / `update` / `publish`**, and only
of those — `install.sh` and `saule run` never touch it. That's the honest cost
of dropping the index, and it's a fair trade: anyone publishing or consuming
git-hosted packages has git.

For GitHub specifically, the CLI takes a fast path: resolve tags via
`git ls-remote`, then download `codeload.github.com/<owner>/<repo>/tar.gz/<sha>`
over plain HTTPS instead of cloning. Same integrity story (the SHA is pinned),
a fraction of the bytes, and no `.git` in `SAULE_HOME`. Other hosts fall back
to a shallow `git clone --depth 1` at the resolved SHA.

### Transitive dependencies

Depth-first over each dependency's own `saule.config`. **One version per import
name per project** — two versions of `uikit` in one build would collide in the
interpreter's module namespace.

With no ranges (§3), the rule that satisfies that constraint is one line:

> A version in a config is a **minimum**. For each package, install the highest
> version any dependency asked for.

Your project pins `uikit 1.2.0`, `toolkit` pins `uikit 1.3.0` → install 1.3.0.
Deterministic, order-independent, and computable in a single pass with no
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

Version-scoped directories in `SAULE_HOME` still matter — two *projects* can
use different versions concurrently. Only within a single build is a single
version enforced.

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
(Ctrl-C, crash, full disk) leaves garbage in `tmp/` and nothing else. `saule
clean` sweeps it.

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

---

## 7. Native packages, without an index

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
`SHA256SUMS` asset, and drops it into `native_packages/` + `native_manifests/` —
the exact layout the install scripts produce today, so
[dynamic_packages.rs](crates/saule-interpreter/src/dynamic_packages.rs)
discovery works unchanged.

If no asset matches the host triple: fall back to building from source **only
when a Rust toolchain is present and the user passes `--build`**, otherwise fail
with the missing triple named. Silent fallback to a source build is a nasty
surprise on a slow machine, and an implicit one is arbitrary code execution at
install time.

`saule publish` on a native package builds the matrix in CI and uploads the
assets as part of step 10 — the author runs the same one command.

Non-GitHub hosts have no release-asset equivalent that's worth special-casing.
Native packages are GitHub-only at first; source packages work anywhere.

---

## 8. Toolchain distribution

**Build matrix** (GitHub Actions, on tag):

| Platform | Triple |
|---|---|
| macOS | `aarch64-apple-darwin`, `x86_64-apple-darwin` |
| Linux | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` |
| Linux (static) | `x86_64-unknown-linux-musl` |
| Windows | `x86_64-pc-windows-msvc` |

Each artifact is `saule-<version>-<triple>.{tar.gz,zip}` containing `saule` and
`saule-lsp`, published to a GitHub Release with a `SHA256SUMS` file.

**Installers** — `install.sh` (POSIX, `curl … | sh`) and `install.ps1`:

1. Detect OS + arch → triple
2. Resolve latest release (or `--version`)
3. Download, **verify checksum**, unpack to `~/.saule/bin`
4. Write `~/.saule/env`, add to PATH via the appropriate shell profile
5. Print a verification command

Checksum verification is not optional — `curl | sh` that installs an unverified
binary is the single riskiest thing in this plan.

macOS notarization: unsigned binaries hit Gatekeeper. Either document the
`xattr -d com.apple.quarantine` workaround or budget for an Apple Developer
account ($99/yr). Worth deciding early; it shapes the macOS first-run experience.

These replace [install_path.sh](scripts/install_path.sh)'s build-from-source
model. The existing `install_mac.sh` / `install_wsl.sh` / `install_windows.ps1`
become **developer** scripts for working on engine-lib locally, and should be
documented as such rather than as the user-facing path.

---

## 9. Editor plugins

All three already fall back to `saule-lsp` on `$PATH` ([SauleToolchain.kt:74](editors/intellij/src/main/kotlin/com/saule/lang/SauleToolchain.kt), [extension.ts:178](editors/vscode/src/extension.ts)), so **once the installer puts the server on PATH, all three work as-is.** The gap is publication, not function.

**VS Code** — needs a real Marketplace publisher ID (`package.json` currently
says `"publisher": "saule"`, which must be claimed or changed), a PAT in CI, and
`vsce publish` on tag. Also worth publishing to OpenVSX for VSCodium users.

**IntelliJ** — needs a JetBrains Marketplace account and token, plus the
`publishPlugin` Gradle task. Two known issues to settle first: the platform
wants `sourceCompatibility=21` while [gradle.properties](editors/intellij/gradle.properties) pins `javaVersion=17`, and `gradlew` was committed non-executable. The LSP4IJ runtime dependency is already declared correctly.

**Neovim** — currently expects the repo checkout: [lsp.lua](editors/nvim/lua/saule/lsp.lua) derives the server path by walking up from its own file. **Change the default to plain `saule-lsp` on PATH**, keeping repo detection as an opt-in for contributors. Then it installs cleanly via lazy.nvim/packer pointing at the repo. Longer term, upstream the filetype and LSP config into `nvim-lspconfig`.

One thing the LSP needs from this work: dependency sources now live in
`SAULE_HOME`, so go-to-definition into a package resolves to a read-only file
outside the workspace. The server already receives `src_dirs` per dependency;
it should mark those roots read-only rather than letting a user edit an
immutable installed package.

---

## 10. Blockers to clear first

1. **Licensing.** No LICENSE file, no `license` field in any `Cargo.toml`, but VS Code's manifest claims MIT. Without an explicit license the code is "all rights reserved" by default, which contradicts that claim. Both Marketplaces require this to be coherent. **Pick a license, add the file, set `license` in the workspace package.** Nothing else here should ship first.
2. **Versioning policy.** Everything is `2026.1.0` — toolchain, plugins, engine-lib, in lockstep. Third-party packages need semver, because §6 compares *major* components to decide what's incompatible and *whole versions* to pick a winner. `saule publish` should therefore require a semver `version:` and reject anything else, which is a rule worth setting before the first package exists. Decide separately whether the toolchain keeps its calendar scheme, and define what `min_saule_version` means when compared against it — [version_at_least](crates/saule-cli/src/project.rs:213) already does a plain numeric dotted compare, which works for both.
3. **UIKit does not exist yet.** It's the motivating package for the whole source-package path and the first real test of `saule publish`. Ship it as a stub early — publishing a package you wrote is the only way to find out whether the flow is actually seamless.
4. **Repo visibility.** Verify the GitHub repo is public before anything points users at it.

---

## 11. Suggested sequence

| # | Milestone | Delivers |
|---|---|---|
| 0 | License, versioning policy, CI skeleton | Legal ability to publish |
| 1 | Release workflow + `install.sh` / `install.ps1` | Users can install and run Saule |
| 2 | nvim PATH default; VS Code + IntelliJ published | Working editor support |
| 3 | `clap` migration + `saule check` | The base every command below builds on |
| 4 | Config/lock format, `add`/`install`/`remove`/`list`, atomic install | Consuming git packages works |
| 5 | `publish`, `init --lib`, `doctor`, `clean` | Anyone can publish; broken states are recoverable |
| 6 | `update`, transitive resolution, `--as` | Real dependency graphs |
| 7 | Native release assets | `saule add <native>` with no Rust toolchain |

Milestones 1 and 2 are independently useful — they deliver a usable public
release before any package-manager work starts, and they're where I'd begin.

Milestone 4 ships **direct dependencies only** — no transitive resolution — if
that gets it out sooner. Because versions are exact pins and the §6 rule is
"highest minimum wins", adding transitive resolution in milestone 6 does not
change the format or invalidate any lockfile written before it.

---

## 12. Things worth deciding now

- **Name collisions are now a local problem, not a global one.** Nobody can
  squat `uikit`, but two packages can both claim it. `--as` handles it. The
  cost is that "the uikit package" is ambiguous in conversation and docs, the
  way it isn't on npm. That's the real price of no index, and it's the right
  price at this scale.
- **Discovery.** With no index there is no `saule search`. A curated
  `awesome-saule` list in the docs is the honest answer for now; a GitHub topic
  (`saule-package`) makes a search possible later without committing to
  infrastructure.
- **`git` as a hard dependency of `add`.** Documented in §6. If that ever
  becomes unacceptable, a GitHub-API-only path covers most cases — but it
  wouldn't cover any other host.
- **Tag mutability.** The lock pins SHAs, which handles it for consumers. Worth
  also having `saule publish` refuse to overwrite an existing tag (step 6) so
  authors don't do it to themselves.
- **No version ranges** (§3). Settled here as a decision, not an omission: with
  a lockfile the caret is inert for direct deps, and §6's "highest minimum wins"
  covers transitive dedup without a requirement grammar. Adding ranges later is
  backwards compatible; removing them would not be.
- **`saule add` running build scripts.** Native packages ship prebuilt.
  `--build` is the only path to executing package-authored build code, and it
  stays explicit.
- **Offline mode.** `saule install --offline` from cache matters for CI, and is
  cheap once `cache/` exists.
