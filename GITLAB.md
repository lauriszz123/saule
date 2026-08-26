# GitLab setup

How this project is hosted, built and released, and what you have to do once to
make each part work.

The short version:

| Concern | Lives on | Why |
|---|---|---|
| Code, issues, merge requests | **GitLab** | Source of truth |
| CI (`cargo test`, fixtures, lints) | **GitLab**, hosted Linux runners | Free, no machine of yours involved |
| Release builds — Linux | **GitLab**, hosted Linux runners | Including the aarch64 cross build |
| Release builds — macOS, Windows | **GitLab**, *your* self-hosted runners | No free hosted runners for either |
| Release archives | **GitLab** package registry + Releases | What `install.sh` downloads |
| Docs site + `install.sh` URL | **GitHub Pages**, unchanged | Keeps the installer URL stable forever |

GitHub keeps exactly two workflows — `deploy-www.yml` and `check-www.yml` —
and is fed by a push mirror. `ci.yml` and `release.yml` were deleted: with
tags mirroring across, leaving `release.yml` in place would build and publish
every release a second time.

---

## 1. Create the project and push

Create an **empty** project at `gitlab.com/lauriszz123/saule` — no README, no
`.gitignore`, no license, or the first push is rejected as a non-fast-forward.
If you pick a different path, change it in three places: `www/site.config.mjs`
(`repo`), `www/public/install.sh` (`GITLAB_PROJECT`) and
`www/public/install.ps1` (`$GitLabProject`).

```bash
git remote rename origin github
git remote add origin git@gitlab.com:lauriszz123/saule.git
git push -u origin main
git push origin --tags
```

Pushing the tags matters: `scripts/next-version.sh` derives the next build
number from them, so without them the next release would restart at `.1` and
collide with a version you already published.

Then set **Settings → General → Visibility** to **Public**. The installer
downloads from the package registry with no token, which only works on a
public project.

## 2. The release token

`cut-release` creates a tag through the API, and `$CI_JOB_TOKEN` deliberately
cannot do that — which is a good property, not an obstacle to route around.

**Settings → Access tokens** → new token, role **Maintainer**, scope **api**.
Copy the value; it is shown once.

**Settings → CI/CD → Variables** → add it:

| | |
|---|---|
| Key | `RELEASE_TOKEN` |
| Value | the token |
| Type | Variable |
| Flags | **Masked**, **Protected** |

Protected means it is only exposed to jobs on protected refs. `main` is
protected by default; make sure the `v*` tag pattern is too, under
**Settings → Repository → Protected tags** (add `v*`, allowed to create:
Maintainers). That also stops anyone but a maintainer from publishing a
release by pushing a tag.

## 3. Self-hosted runners for macOS and Windows

Everything Linux runs on GitLab's hosted runners. macOS and Windows do not
have a free hosted option, so both come from machines you own. Get the
registration token from **Settings → CI/CD → Runners → New project runner**.

Both runners need: `git`, and a `rustup` toolchain on stable.

### macOS

One Apple Silicon Mac covers both Apple triples — Rust cross-compiles to
`x86_64-apple-darwin` out of the box, and the result still runs locally under
Rosetta, so the pipeline's version check is real for both archives.

```bash
brew install gitlab-runner
gitlab-runner register --url https://gitlab.com --token <token> \
  --executor shell --tag-list saule-macos --description "saule macOS"
brew services start gitlab-runner
```

Use the **shell** executor, not docker: a Docker container on macOS is Linux,
which cannot produce a Mach-O binary.

### Windows

Needs the **MSVC build tools** as well, since the target is
`x86_64-pc-windows-msvc` — the Visual Studio Build Studio installer's "Desktop
development with C++" workload is enough. `Compress-Archive` handles the zip,
so no 7-Zip.

In an elevated PowerShell:

```powershell
New-Item -ItemType Directory -Force C:\GitLab-Runner
Invoke-WebRequest -UseBasicParsing `
  "https://gitlab-runner-downloads.s3.amazonaws.com/latest/binaries/gitlab-runner-windows-amd64.exe" `
  -OutFile C:\GitLab-Runner\gitlab-runner.exe
cd C:\GitLab-Runner
.\gitlab-runner.exe register --url https://gitlab.com --token <token> `
  --executor shell --shell powershell --tag-list saule-windows --description "saule Windows"
.\gitlab-runner.exe install
.\gitlab-runner.exe start
```

`--shell powershell` is required: the `build:windows` job is a PowerShell
script, and the runner's default on Windows is not guaranteed to be one.

Both runners: turn **off** "run untagged jobs" in the runner's settings. They
are pinned to `saule-macos` / `saule-windows` by tag, and an untagged runner
would otherwise start picking up the Linux jobs and failing them.

## 4. Cutting a release

**CI/CD → Pipelines → Run pipeline** on `main`, then press ▶ on the
`cut-release` job. It works out the next build number from the existing tags,
checks it against the year in `Cargo.toml`, and creates the tag. That is all it
does.

The tag then starts the real release pipeline: six builds, each of which
*executes* its binary and checks it reports the expected version, then upload
to the package registry and a GitLab Release.

Two things to know:

- **The tag is the handoff.** GitHub Actions suppresses pipelines for events
  raised by its own token; GitLab does not. Splitting tag-creation from
  publishing is what stops a job from racing with the pipeline its own tag
  started.
- **Push `v26.7` by hand** to skip `cut-release` entirely. That is the escape
  hatch for re-cutting a release whose build failed for an infrastructure
  reason.

To rehearse without publishing, run a pipeline on `main` with
`RELEASE_DRY_RUN` = `true`. It builds all six and publishes nothing.

Version numbers are `<year>.<build>` — `26.7`, no patch component. In January,
bump `26` to `27` in `Cargo.toml`'s `[workspace.package] version` and nowhere
else; `cut-release` refuses to build a version whose year disagrees with it.

## 5. The GitHub mirror

The docs site stays on GitHub Pages, which is what keeps
`https://lauriszz123.github.io/saule/install.sh` working forever regardless of
where the code lives. GitHub therefore still needs the commits.

**Settings → Repository → Mirroring repositories**:

| | |
|---|---|
| Git repository URL | `https://<your-github-username>@github.com/lauriszz123/saule.git` |
| Mirror direction | Push |
| Password | a GitHub personal access token with `repo` scope |

Tick **Keep divergent refs** off and **Mirror only protected branches** off —
the site build reads `main`, and the examples check wants the tags.

Push mirroring is free on GitLab. (Pull mirroring is not, which is the other
reason the direction is GitLab → GitHub rather than the reverse.)

After the first mirror push, check that GitHub Actions still shows only
"Deploy website" and "Check website". If "CI" or "Release" appear, the deletion
in this commit has not reached GitHub yet.

## 6. Verifying the whole thing once

```bash
# 1. A dry run proves all six platforms build and self-verify.
#    CI/CD → Run pipeline on main, RELEASE_DRY_RUN=true

# 2. Cut a real release, then confirm the installer sees it:
curl -fsSL "https://gitlab.com/api/v4/projects/lauriszz123%2Fsaule/releases/permalink/latest"

# 3. Install into a scratch directory rather than over your real toolchain:
SAULE_HOME=/tmp/saule-test SAULE_NO_MODIFY_PATH=1 \
  sh -c "$(curl -fsSL https://lauriszz123.github.io/saule/install.sh)"
/tmp/saule-test/bin/saule --version
```

Step 3 only works once the site has redeployed through the GitHub mirror, since
that is where `install.sh` is served from.
