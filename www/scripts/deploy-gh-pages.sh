#!/usr/bin/env bash
# Build the site locally and publish it to the `gh-pages` branch.
#
# This is the fallback for when GitHub Actions cannot run — an account billing
# lock, a self-hosted setup, or simply wanting to ship without CI. It produces
# exactly the same site the workflow would; the only difference is that your
# machine does the building.
#
# Usage:
#   www/scripts/deploy-gh-pages.sh            # build, commit, and push
#   www/scripts/deploy-gh-pages.sh --dry-run  # build and stage, but don't push
#
# One-time setup after the first successful run:
#   Settings > Pages > Build and deployment > Source > "Deploy from a branch"
#   Branch: gh-pages / (root)
#
# Note this is the *other* Pages mode from the workflow in
# .github/workflows/deploy-www.yml — pick one. If you later fix billing and go
# back to Actions, switch the Source back to "GitHub Actions"; leaving it on
# the branch means pushes to main stop updating the site.
set -euo pipefail

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
	DRY_RUN=true
fi

WWW_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REPO_ROOT="$(cd "$WWW_DIR/.." && pwd)"
WORKTREE="$REPO_ROOT/.gh-pages-worktree"
BRANCH="gh-pages"

cd "$REPO_ROOT"

if [[ -n "$(git status --porcelain)" ]]; then
	echo "warning: the working tree has uncommitted changes." >&2
	echo "         The site is built from your files on disk, not from HEAD," >&2
	echo "         so those changes will be published." >&2
	echo >&2
fi

SHA="$(git rev-parse --short HEAD)"

echo "==> Building the site"
cd "$WWW_DIR"
npm run sync-docs
# `npm run build` triggers `prebuild`, which compiles crates/saule-wasm and
# runs wasm-bindgen — the playground's runtime is produced here, not committed.
npm run build

if [[ ! -f "$WWW_DIR/dist/index.html" ]]; then
	echo "error: build produced no dist/index.html" >&2
	exit 1
fi

echo "==> Preparing the $BRANCH worktree"
cd "$REPO_ROOT"

# A stale worktree from an interrupted run would block `worktree add`.
git worktree remove --force "$WORKTREE" 2>/dev/null || true
rm -rf "$WORKTREE"

# The site is published to GitHub Pages, so the deploy target is the `github`
# remote — *not* `origin`, which has pointed at GitLab since the move (see
# GITLAB.md). Getting this wrong is not a harmless failure: `ls-remote` against
# a remote you cannot read fails identically to "the branch does not exist", so
# the fallback below would quietly rebuild `gh-pages` from HEAD and bury the
# published site under the entire source history.
PAGES_REMOTE="${PAGES_REMOTE:-github}"

if ! git remote get-url "$PAGES_REMOTE" >/dev/null 2>&1; then
	echo "error: no git remote named '$PAGES_REMOTE'." >&2
	echo "       Set PAGES_REMOTE=<name> to whichever remote hosts Pages." >&2
	exit 1
fi

# Distinguish "cannot reach the remote" from "the branch is not there yet".
# Only the second one may create the branch.
if ! git ls-remote --heads "$PAGES_REMOTE" >/dev/null 2>&1; then
	echo "error: cannot read from remote '$PAGES_REMOTE'." >&2
	echo "       Fix access before deploying — continuing would rewrite" >&2
	echo "       $BRANCH from scratch instead of building on the live site." >&2
	exit 1
fi

# Track the remote branch if it exists; otherwise start the branch here.
if git ls-remote --exit-code --heads "$PAGES_REMOTE" "$BRANCH" >/dev/null 2>&1; then
	git fetch "$PAGES_REMOTE" "$BRANCH"
	git worktree add -B "$BRANCH" "$WORKTREE" "$PAGES_REMOTE/$BRANCH"
else
	# First deploy: an *orphan* branch, so the published site carries only its
	# own deploy history and not a copy of the source tree's.
	echo "    (no $BRANCH on $PAGES_REMOTE yet — creating it)"
	git worktree add --detach "$WORKTREE"
	git -C "$WORKTREE" checkout --orphan "$BRANCH"
	git -C "$WORKTREE" rm -rf --cached . >/dev/null 2>&1 || true
fi

echo "==> Copying the build"
# Clear everything except .git, so files deleted from the site disappear from
# the branch too rather than lingering forever.
find "$WORKTREE" -mindepth 1 -maxdepth 1 ! -name '.git' -exec rm -rf {} +

cp -R "$WWW_DIR/dist/." "$WORKTREE/"

# Branch-based Pages runs the published files through Jekyll, which silently
# drops every directory whose name starts with an underscore. Astro puts all
# of its CSS and JS in `_astro/`, so without this file the site loads as
# unstyled HTML with no working scripts. The Actions deployment path does no
# Jekyll processing and does not need it — which is why `.nojekyll` is created
# here rather than committed under www/public/.
touch "$WORKTREE/.nojekyll"

cd "$WORKTREE"
git add --all

if git diff --cached --quiet; then
	echo "==> No changes to publish; the branch already matches this build."
	cd "$REPO_ROOT"
	git worktree remove --force "$WORKTREE"
	exit 0
fi

git commit -m "Deploy website from $SHA"

if [[ "$DRY_RUN" == true ]]; then
	echo
	echo "==> --dry-run: committed to $BRANCH but not pushed."
	echo "    Inspect it:  git -C $WORKTREE show --stat"
	echo "    Then push:   git -C $WORKTREE push $PAGES_REMOTE $BRANCH"
	echo "    Clean up:    git worktree remove --force $WORKTREE"
	exit 0
fi

echo "==> Pushing $BRANCH to $PAGES_REMOTE"
git push "$PAGES_REMOTE" "$BRANCH"

cd "$REPO_ROOT"
git worktree remove --force "$WORKTREE"

echo
echo "Published. If this is the first deploy, set:"
echo "  Settings > Pages > Source > Deploy from a branch > $BRANCH / (root)"
echo "Then the site appears at https://lauriszz123.github.io/saule/"
