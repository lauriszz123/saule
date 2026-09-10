# CircleCI setup

Everything CI does for Saule, and the one-time setup behind it. The pipeline
itself is [.circleci/config.yml](.circleci/config.yml), which is commented at
least as heavily as this file — read it for *why* a job is shaped the way it
is; this is *what you have to click once*.

| Concern | Lives on | Why |
|---|---|---|
| Code, issues, pull requests | **GitHub** | Source of truth |
| CI (`cargo test`, fixtures, lints) | **CircleCI**, Docker | Free, no machine of yours involved |
| Release builds — Linux ×3 | **CircleCI**, Docker | aarch64 is cross-compiled and qemu-verified |
| Release builds — macOS ×2, Windows | **CircleCI**, hosted macOS and Windows | Both are on the free plan's credits |
| Release archives | **GitHub** Releases | What `install.sh` downloads |
| Docs site + installer URL | **GitHub Pages** | Keeps the installer URL stable forever |

The split matters in one direction only: the installer downloads from
`github.com/lauriszz123/saule/releases/download/…`, which is an anonymous
download with no rate limit and a URL that never changes. Nothing a user
touches knows which CI built the binary.

---

## The branch policy

**`main` publishes. `develop` does nothing.**

- **Push or merge to `main`** → tests run; if they pass, all six archives are
  built and a GitHub Release is created, tag included. The version generates
  itself (`scripts/next-version.sh` takes the next build number after the
  highest existing `v26.*` tag), so nobody types a version number.
  `install.sh` sees the new release as `latest` the moment it exists.
- **Push to `develop`** (or any other branch) → nothing runs. The single
  workflow's `when:` matches only `main` and the `dry-run` parameter, so a
  pipeline is created with no jobs in it and costs nothing.
- **`[skip ci]` in a commit message** → not even a pipeline. This is the hatch
  for a `main` push that should not cut a release: a typo in a comment, a
  README tweak.

Create `develop` and work there:

```bash
git switch -c develop
git push -u github develop
```

A push to `develop` still *creates* a pipeline, it just contains no jobs. If
those empty entries in the dashboard annoy you, narrow the trigger itself:
**Project Settings → Triggers →** edit the GitHub trigger to run on pushes to
`main` only. That is cosmetic — an empty pipeline burns no credits.

---

## 1. The project connection

**If the project is already connected on CircleCI, leave it alone.** A project
is a link between CircleCI and the GitHub repository; it holds no pipeline
definition of its own. Every run reads `.circleci/config.yml` out of the
commit it is building, so replacing that file is the whole migration — there
is nothing to re-create, re-import, or re-authorise.

Two leftovers from the onboarding are worth clearing:

1. **The `circleci-project-setup` branch.** CircleCI's setup wizard pushed a
   generated config there (build-node / test-java / test-rust against
   `cimg/rust:1.70`). It is superseded, and leaving it invites someone to
   merge it back:

   ```bash
   git push github --delete circleci-project-setup
   ```

2. **The trigger's branch scope.** **Project Settings → Triggers** — if the
   wizard set it to build `circleci-project-setup`, or only that branch, point
   it at the repository's default branch (or all branches; the config's own
   `when:` is what keeps `develop` idle either way).

Connecting from scratch, if it ever comes to that: **circleci.com → Projects →
Create Project → `lauriszz123/saule`**, then "Fastest: use an existing config".

**GitHub Actions is not involved and does not need to be unblocked.** There are
no workflows left in the repository — the two website ones were deleted along
with `.github/`. GitHub keeps three jobs and no more: hosting the code, hosting
the Releases, and serving Pages.

One consequence worth pinning down: **Settings → Pages → Source must be "Deploy
from a branch" → `gh-pages` / (root)**. The other mode ("GitHub Actions") now
has no workflow behind it, and the installer one-liners are served from that
site.

---

## 2. The release token

The publish job is the only job that writes anything outside CircleCI, and the
only one that needs a credential.

1. GitHub → **Settings → Developer settings → Personal access tokens → Fine
   grained tokens → Generate new token**.
   - Repository access: **only** `lauriszz123/saule`
   - Permissions → Repository → **Contents: Read and write** (this is what
     creates tags and releases). Nothing else.
   - Expiry: whatever you will actually remember to rotate.
2. CircleCI → **Organization Settings → Contexts → Create Context**, named
   **`saule-release`** (the name is in `config.yml`; change both or neither).
3. Add an environment variable to it:

   | | |
   |---|---|
   | Name | `GITHUB_TOKEN` |
   | Value | the token |

4. Restrict the context to a security group if the org has one. Only the
   `publish` job requests it, so nothing else in the pipeline can read it.

If the token is missing the publish job stops with a message saying so rather
than half-creating a release.

---

## 3. Before the first run: clear the stale `v26.1` tag

`v26.1` was tagged and pushed on 2026-07-30 and never built or released — it
predates all of this and sits 104 commits behind `main`. While it exists the
first release this pipeline cuts is `26.2`, because the build number is one
past the highest existing tag.

The first release should be **`26.1`**, so delete it — locally and on GitHub —
before the first push to `main`:

```bash
git tag -d v26.1 && git push github --delete v26.1
```

With no tags left, `scripts/next-version.sh` yields `26.1`, and local builds in
the meantime report `26.1-dev+<sha>` — the same number they are heading toward.
Do not try to publish the old tag instead: a build from today's `main` would be
labelled with a commit from weeks ago.

---

## 4. Cutting a release

Merge to `main`. That is the whole procedure.

```bash
git switch main
git merge --no-ff develop
git push github main
```

The pipeline then: runs the tests → resolves the version → builds six archives,
each one *executed* to confirm it reports the version the release claims →
checksums them and refuses a partial set → creates the tag and the Release and
uploads seven assets.

### Where the version comes from, and why nothing is committed back

The number advances on its own, once per release, and **no commit to `main` is
involved**:

1. The `version` job runs `scripts/next-version.sh`, which reads the `v26.*`
   tags that exist and takes the next build number — `26.1`, then `26.2`, then
   `26.3`. It writes that one string into the pipeline's workspace, so all six
   builds agree on it even if something else lands mid-pipeline.
2. Each build exports it as `$SAULE_VERSION`, which
   [crates/saule-version/build.rs](crates/saule-version/build.rs) bakes into
   the binaries. `saule --version`, `saule-lsp --version` and `Saule.version`
   all read it back out of the compiled binary.
3. The publish job creates the tag. **The tag is where the number is stored**,
   which is what makes the next run compute the next number.

So the list of published tags *is* the version state. Nothing writes a version
into `Cargo.toml`, and CI never pushes a commit — a CI that commits to the
branch that triggers it is how you get a pipeline that triggers itself.
`[workspace.package] version = "26.0.0"` in `Cargo.toml` carries exactly one
piece of information, the **year**, and it is the one line to change each
January.

Between releases a local build reports `26.4-dev+1a2b3c4` — the number it is
heading toward, marked dev, so a development binary can never be mistaken for
a release.

### Rehearsing first — do this before the first real one

A dry run builds and verifies all six platforms and publishes nothing. From
the CircleCI UI: **Trigger Pipeline**, add parameter `dry-run` = `true`, and
pick whichever branch you want built. It works from `develop` too, which is
the way to prove a change on all six platforms before merging.

The classic API form, for a project on the GitHub OAuth integration:

```bash
curl -X POST https://circleci.com/api/v2/project/gh/lauriszz123/saule/pipeline \
  -H "Circle-Token: $CIRCLE_TOKEN" -H 'content-type: application/json' \
  -d '{"branch":"develop","parameters":{"dry-run":true}}'
```

A project on the newer GitHub App integration uses
`/api/v2/project/<project-id>/pipeline/run` with a `definition_id` instead;
the UI button is easier than looking those up.

### Re-cutting after an infrastructure failure

If a build fails for a reason that is not the code — a runner dies, a network
blip — just re-run the pipeline from CircleCI. Nothing was published, so
nothing is half-done, and the next attempt takes the same version number.

If a tag exists but the Release does not, pass the version explicitly:
`release-version` = `26.2`. That path is allowed to reuse an existing tag,
which the auto-generated path deliberately is not. It still refuses if a
Release exists for that tag: **a published version is never rebuilt.**

---

## 5. Verifying the whole thing once

```bash
# 1. A dry run proves all six platforms build and self-verify.
#    Trigger Pipeline → dry-run = true

# 2. Merge to main and let it publish, then confirm the installer sees it:
curl -sI https://github.com/lauriszz123/saule/releases/latest | grep -i location

# 3. Install into a scratch directory rather than over your real toolchain:
SAULE_HOME=/tmp/saule-test SAULE_NO_MODIFY_PATH=1 \
  sh -c 'curl -fsSL https://lauriszz123.github.io/saule/install.sh | sh'
/tmp/saule-test/bin/saule --version
/tmp/saule-test/bin/saule-lsp --version
```

Then the real check, and the one the plan calls "done": a machine that has
never had Saule on it — a fresh VM, a colleague's laptop, one of each OS —
going from nothing to a working `saule --version` with one command.

---

## 6. What this costs

The free plan covers all of it, but macOS and Windows draw on a smaller pool
(30,000 credits/month) than Linux and Docker do. A release builds two macOS
archives and one Windows archive, and the release profile is fat LTO with one
codegen unit, so those three jobs are the expensive part of the month.

That is the reason `[skip ci]` exists in the branch policy above: with `main`
publishing on every push, a habit of merging documentation fixes straight to
`main` is a habit of spending macOS credits on them. Batch work on `develop`
and merge in meaningful chunks.

Two knobs if it ever matters:

- `resource_class` on `build-linux` (currently the default `medium`) — raising
  it costs credits, lowering the wall clock.
- Dropping `x86_64-apple-darwin`. Intel Macs are the one platform in the
  matrix with a shrinking population, and it is the archive whose runtime
  check depends on Rosetta being present on the builder (see the warning path
  in `.circleci/build-unix.sh`).

---

## 7. Things the pipeline deliberately does not do

- **`cargo fmt --all --check` does not fail the build.** The tree carries
  formatting drift that predates any CI, and `main` publishes — a blocking
  gate would mean no release could be cut until the entire backlog was cleared
  in one commit. It reports the drift and moves on. Clear it crate by crate
  (not with a blanket `cargo fmt --all`: some files here are CRLF and a
  wholesale reformat rewrites their line endings too), then make the step
  blocking. clippy and the test suites *are* blocking, and a red one means no
  archives are built and no release is created.

- **No tag-push pipelines.** The publish job creates the tag through the GitHub
  API, so there is exactly one publisher and nothing to race with. Pushing a
  `v26.7` tag by hand starts nothing.
- **No release builds on pull requests.** Use `dry-run` for that.
- **No website deploy.** The site is still published by hand with
  `www/scripts/deploy-gh-pages.sh` (`.ps1` on Windows). CI *checks* the site —
  samples compile, generated docs are in sync, Astro builds — but does not
  publish it. Automating that needs a git-push credential in CI, which is a
  bigger blast radius than the release token, and the installer URL only
  changes when the scripts do.
- **No editor plugin publishing.** VS Code, IntelliJ and Neovim are
  RELEASE_PLAN step 4, and all three need marketplace accounts and tokens
  before CI can do anything for them. Run `scripts/stamp-version.sh <version>`
  and commit before packaging either of the first two.
