# Saule — Public Release & Package Distribution Plan

Status: **proposal, nothing implemented.** Written against `05c481f`.

Goal: a user on macOS, Linux, or Windows runs one command, gets the Saule
toolchain and working editor support, and can then `saule add uikit` to pull
in packages.

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

Two things are already right and shape the design:

- **`SAULE_HOME` is now a well-defined install root** ([dynamic_packages.rs:100](crates/saule-interpreter/src/dynamic_packages.rs:100)) — the natural home for packages, binaries, and docs.
- **Dependencies are resolved in the CLI, not the interpreter.** [project.rs](crates/saule-cli/src/project.rs) turns each `dependencies:` entry into a `Dependency { name, root, src_dirs }` and hands it to the interpreter. The interpreter never learns where a dependency came from.

That second point is the key architectural lever: **installed source packages need no interpreter changes at all.** `saule add uikit` installs source into `SAULE_HOME`, and `project.rs` resolves it into the same `Dependency` struct a relative path produces today. [module.rs](crates/saule-interpreter/src/module.rs) is untouched.

---

## 2. `SAULE_HOME` layout

Additive — the two existing directories keep their names and meaning, so
current installs stay valid.

```
~/.saule/
├── bin/                     saule, saule-lsp            (new)
├── packages/                source packages             (new)
│   └── uikit/1.2.0/         unpacked, immutable
├── native_packages/         compiled cdylibs            (exists)
├── native_manifests/        TOML manifests              (exists)
├── registry/                cached package index        (new)
├── docs/                    offline docs                (new)
└── env                      sourceable PATH snippet     (new)
```

Version-scoped package directories mean two projects can depend on different
versions of `uikit` without conflict — worth having from day one, since
retrofitting it later is painful.

---

## 3. Package manifest & dependency format

`saule.config` is deliberately minimal — `key: "value"` lines, `--` comments,
list values. It has **no nesting**, which versioned dependencies would
normally need.

**Recommendation: keep the format, extend the dependency microformat.** A TOML
migration would break every existing project for little gain, and the parser at
[project.rs:read_config](crates/saule-cli/src/project.rs) stays simple.

```
dependencies: [
  "../json",                    -- local path (unchanged, still works)
  "uikit@^1.2",                 -- registry package
  "github:user/repo@v0.3.1",    -- direct git, bypasses the index
]
```

Resolution order per entry: contains `/` or starts with `.`/`~` → local path
(today's behavior, bit-for-bit). Starts with a scheme (`github:`) → direct git.
Otherwise → registry name.

### Lockfile

New `saule.lock`, committed by the user:

```
uikit 1.2.0 git=https://github.com/... rev=<sha> sha256=<digest>
```

Records the **resolved commit SHA and content hash**, not just the version, so
a moved tag cannot silently change what a build gets. `saule install` is
reproducible; `saule update` is the only command that rewrites the lock.

---

## 4. CLI surface

New subcommands in [main.rs](crates/saule-cli/src/main.rs)'s dispatch:

| Command | Behavior |
|---|---|
| `saule add <pkg>[@ver]` | Resolve, download, verify, install; update `saule.config` + `saule.lock`. |
| `saule remove <pkg>` | Drop from config and lock. |
| `saule install` | Install exactly what the lock pins. The CI/fresh-clone command. |
| `saule update [pkg]` | Re-resolve within version constraints; rewrite lock. |
| `saule list` | Show installed packages and versions. |
| `saule publish` | Validate, tag, push, create the GitHub release. |
| `saule self update` | Replace the toolchain in place. |

The current dispatch is a hand-rolled `match` on `args[0]`. Seven more commands
with flags and subcommands is where that stops being maintainable — **adopt
`clap`** as part of this work. It also gives real `--help` per subcommand.

---

## 5. Registry: Git/GitHub-backed

No server to run, host, or secure.

**Index repo** (`lauriszz123/saule-index`), one file per package:

```
name = "uikit"
repo = "https://github.com/lauriszz123/uikit"
[[versions]]
version = "1.2.0"
rev = "<full-commit-sha>"
sha256 = "<tarball-digest>"
```

The CLI fetches the index as a **tarball over HTTPS** rather than shelling out
to `git` — no git dependency on the user's machine, and it caches cleanly under
`registry/`.

Name ownership = a PR against the index. Manual at first, which is fine at
current scale and defers the hard trust questions.

### Source packages (UIKit)

Download the tarball at the pinned `rev`, verify `sha256`, unpack to
`packages/<name>/<version>/`. Pure Saule, so it is platform-independent and
needs no build step. `project.rs` then resolves it to a `Dependency` whose
`src_dirs` come from the package's own `saule.config`.

### Native packages (saule-engine-lib)

Same index entry plus per-target release assets:

```
saule_engine_lib-1.2.0-aarch64-apple-darwin.dylib
saule_engine_lib-1.2.0-x86_64-unknown-linux-gnu.so
saule_engine_lib-1.2.0-x86_64-pc-windows-msvc.dll
engine.toml
```

The CLI picks the asset matching the host triple, verifies its digest, and
drops it into `native_packages/` + `native_manifests/` — the exact layout the
install scripts produce today, so discovery already works unchanged.

If no asset matches the host triple, fall back to building from source when a
Rust toolchain is present, and otherwise fail with an explicit message. Silent
fallback to a source build would be a nasty surprise on a slow machine.

---

## 6. Toolchain distribution

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

## 7. Editor plugins

All three already fall back to `saule-lsp` on `$PATH` ([SauleToolchain.kt:74](editors/intellij/src/main/kotlin/com/saule/lang/SauleToolchain.kt), [extension.ts:178](editors/vscode/src/extension.ts)), so **once the installer puts the server on PATH, all three work as-is.** The gap is publication, not function.

**VS Code** — needs a real Marketplace publisher ID (`package.json` currently
says `"publisher": "saule"`, which must be claimed or changed), a PAT in CI, and
`vsce publish` on tag. Also worth publishing to OpenVSX for VSCodium users.

**IntelliJ** — needs a JetBrains Marketplace account and token, plus the
`publishPlugin` Gradle task. Two known issues to settle first: the platform
wants `sourceCompatibility=21` while [gradle.properties](editors/intellij/gradle.properties) pins `javaVersion=17`, and `gradlew` was committed non-executable. The LSP4IJ runtime dependency is already declared correctly.

**Neovim** — currently expects the repo checkout: [lsp.lua](editors/nvim/lua/saule/lsp.lua) derives the server path by walking up from its own file. **Change the default to plain `saule-lsp` on PATH**, keeping repo detection as an opt-in for contributors. Then it installs cleanly via lazy.nvim/packer pointing at the repo. Longer term, upstream the filetype and LSP config into `nvim-lspconfig`.

---

## 8. Blockers to clear first

1. **Licensing.** No LICENSE file, no `license` field in any `Cargo.toml`, but VS Code's manifest claims MIT. Without an explicit license the code is "all rights reserved" by default, which contradicts that claim. Both Marketplaces require this to be coherent. **Pick a license, add the file, set `license` in the workspace package.** Nothing else here should ship first.
2. **Versioning policy.** Everything is `2026.1.0` — toolchain, plugins, engine-lib, in lockstep. Once packages version independently, decide whether the calendar scheme applies to them too, and what `min_saule_version` means against it.
3. **UIKit does not exist yet.** It's the motivating package for the whole source-package path, so the format should be validated against it early — even as a stub.
4. **Repo visibility.** Verify the GitHub repo is public before anything points users at it.

---

## 9. Suggested sequence

| # | Milestone | Delivers |
|---|---|---|
| 0 | License, versioning policy, CI skeleton | Legal ability to publish |
| 1 | Release workflow + `install.sh` / `install.ps1` | Users can install and run Saule |
| 2 | nvim PATH default; VS Code + IntelliJ published | Working editor support |
| 3 | `clap` migration, manifest/lock format, `add`/`install`/`remove` | Source packages; UIKit ships |
| 4 | Native asset resolution | `saule add engine` with no Rust toolchain |
| 5 | Index repo + `saule publish` | Third parties can publish |

Milestones 1 and 2 are independently useful — they deliver a usable public
release before any package-manager work starts, and they're where I'd begin.

---

## 10. Things worth deciding now

- **Scoped names?** `@lauriszz/uikit` vs flat `uikit`. Flat is friendlier; scoped avoids land-grabs. Changing later is breaking.
- **Yanking.** Some way to mark a bad version unusable without deleting it.
- **`saule add` running build scripts.** Native packages ship prebuilt here. If source builds are ever allowed, that's arbitrary code execution at install time and needs an explicit opt-in.
- **Offline mode.** `saule install --offline` from cache matters for CI.
