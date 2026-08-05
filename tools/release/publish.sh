#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Create the GitHub repository and push to it, once.
#
#     tools/release/publish.sh <owner>/<repo>
#     tools/release/publish.sh <owner>/<repo> --private
#
# Everything a repository needs on the day it appears — description, topics,
# homepage, which tabs are on — set here rather than clicked in a browser, so
# that it is reviewable, repeatable, and in the history alongside everything
# else. A setting nobody can see the reasoning for is a setting nobody will
# remember to keep.
#
# What this does NOT do, deliberately:
#
#   - authenticate. `gh auth login` is yours. No script here should ever be in a
#     position to see a token.
#   - force anything. If the repository already exists it stops, because the
#     one thing worse than not publishing is overwriting something already out
#     there.
#   - set the social preview image. GitHub has no API for it; it is
#     Settings → General → Social preview, and assets/logo.png is what to
#     upload.
#
# Run the checks first. This pushes the whole history to a public address and the
# cheapest moment to notice a problem is before that, not after — see
# tools/ci/check-repo-url.py for what happens when the address is wrong in one
# place out of eleven.

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

TARGET="${1:?usage: publish.sh <owner>/<repo> [--private]}"
VISIBILITY="--public"
[ "${2:-}" = "--private" ] && VISIBILITY="--private"

OWNER="${TARGET%%/*}"
REPO="${TARGET##*/}"
if [ "$OWNER" = "$TARGET" ] || [ -z "$REPO" ]; then
  echo "!! expected <owner>/<repo>, got: $TARGET" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# What the world sees first. 350 characters is GitHub's limit and the first
# ~150 are what a search result shows, so the words that matter go early.
# ---------------------------------------------------------------------------
DESCRIPTION="Free, open-source anti-detect browser. A Chromium 150 fork that spoofs the fingerprint in C++ rather than with injected JavaScript, with per-profile personas, proxies, and a self-hostable team server with per-project access. No seats, no per-profile pricing, no telemetry."

HOMEPAGE=""

# Topics are how a repository is found by somebody who does not know its name.
# GitHub allows 20; each is lowercase, hyphenated, and indexed both on its own
# topic page and by search engines. Both spellings of anti-detect are here on
# purpose — the industry writes it three ways and people search for what they
# write.
TOPICS=(
  antidetect-browser
  anti-detect-browser
  antidetect
  browser-fingerprinting
  browser-fingerprint
  fingerprint-spoofing
  anti-fingerprinting
  chromium-fork
  chromium
  multi-accounting
  privacy
  browser-automation
  self-hosted
  proxy-manager
  canvas-fingerprint
  webgl-fingerprint
  devtools-protocol
  web-scraping
  rust
  tauri
)

say() { printf '==> %s\n' "$*"; }
die() { printf '!! %s\n' "$*" >&2; exit 1; }

# gh may not be on PATH. It is commonly installed outside one — Homebrew's
# prefix differs between Intel and Apple silicon, and a tarball unpacked by
# hand usually lands in ~/.local/bin, which macOS does not add to PATH by
# default. Looking in the obvious places beats telling somebody to edit their
# shell profile before they can publish.
GH="$(command -v gh || true)"
for candidate in "$HOME/.local/bin/gh" /opt/homebrew/bin/gh /usr/local/bin/gh; do
  [ -n "$GH" ] && break
  [ -x "$candidate" ] && GH="$candidate"
done
[ -n "$GH" ] || die "gh is not installed. See the note at the top of this file."

"$GH" auth status >/dev/null 2>&1 || die "gh is not authenticated. Run:
     $GH auth login"

# ---------------------------------------------------------------------------
say "checking before publishing — this is the last cheap moment"
# ---------------------------------------------------------------------------
[ -z "$(git status --porcelain)" ] || die "the working tree is dirty; commit or stash first"

for check in check-repo-url check-commit-identity check-hunks check-duplicate-creates; do
  printf '    %-26s ' "$check"
  python3 "tools/ci/$check.py" >/dev/null || die "$check failed — run it directly to see why"
  echo ok
done
printf '    %-26s ' "deploy manifest"
python3 tools/ci/gen-deploy-workspace.py --check >/dev/null || die "deploy/workspace.toml is stale"
echo ok

# The address in the code has to be the address being published to, or the
# auto-updater queries a repository that does not exist and reports, correctly
# and uselessly, that there is nothing to update.
CODED="$(grep -oE 'github\.com/[A-Za-z0-9-]+/[A-Za-z0-9._-]+' Cargo.toml | head -1 | sed 's|github\.com/||')"
if [ "$CODED" != "$TARGET" ]; then
  die "Cargo.toml says the repository is $CODED but you are publishing to $TARGET.
   Change it everywhere first — tools/ci/check-repo-url.py lists every place —
   or the updater and the download links point at different repositories and
   neither reports an error."
fi

if "$GH" repo view "$TARGET" >/dev/null 2>&1; then
  die "$TARGET already exists. Refusing to touch it."
fi

# ---------------------------------------------------------------------------
say "creating $TARGET (${VISIBILITY#--})"
# ---------------------------------------------------------------------------
# No --add-readme, no --gitignore, no --license: all three are already in the
# repository, and letting GitHub create its own would put a commit at the root
# that the local history does not have, which makes the first push a conflict.
"$GH" repo create "$TARGET" "$VISIBILITY" --description "$DESCRIPTION" ${HOMEPAGE:+--homepage "$HOMEPAGE"}

say "pushing $(git rev-list --count HEAD) commits"
git remote get-url origin >/dev/null 2>&1 && git remote remove origin
git remote add origin "https://github.com/$TARGET.git"
git push -u origin main

# ---------------------------------------------------------------------------
say "topics"
# ---------------------------------------------------------------------------
"$GH" api -X PUT "repos/$TARGET/topics" \
  -H "Accept: application/vnd.github+json" \
  -f "names[]=$(IFS=,; echo "${TOPICS[*]}")" >/dev/null 2>&1 ||
"$GH" repo edit "$TARGET" $(printf -- '--add-topic %s ' "${TOPICS[@]}")
say "  ${#TOPICS[@]} set"

# ---------------------------------------------------------------------------
say "settings"
# ---------------------------------------------------------------------------
# Issues on: the whole point is that people can report a fingerprint that gives
# a profile away, and SECURITY.md sends the private ones elsewhere.
#
# Wiki off: documentation lives in docs/ and is versioned with the code it
# describes. A wiki is a second copy that drifts and that no pull request
# touches.
#
# Projects off: nothing uses it, and an empty tab is a question a visitor has to
# answer for themselves.
#
# Squash-merge only, with the branch deleted after: the history is the design
# record here — the series file, the commit messages, the reasoning — and a
# merge commit per pull request buries it.
"$GH" repo edit "$TARGET" \
  --enable-issues \
  --enable-wiki=false \
  --enable-projects=false \
  --enable-discussions \
  --enable-squash-merge \
  --enable-merge-commit=false \
  --enable-rebase-merge=false \
  --delete-branch-on-merge \
  --allow-update-branch

# Vulnerability alerts on the dependency graph. Free, and this repository ships
# 623 crates and 132 npm packages.
"$GH" api -X PUT "repos/$TARGET/vulnerability-alerts" >/dev/null 2>&1 || true
"$GH" api -X PUT "repos/$TARGET/automated-security-fixes" >/dev/null 2>&1 || true

echo
say "published: https://github.com/$TARGET"
echo
echo "    Two things a script cannot do, both in Settings → General:"
echo "      - Social preview: upload assets/logo.png (1280×640 is what GitHub"
echo "        shows in link previews on every other site)"
echo "      - the repository's own About panel already has the description and"
echo "        topics set above; add a homepage there if one ever exists"
echo
echo "    And check the Actions tab. CI runs on push, so the first result is"
echo "    already on its way, and a red tick on day one is worth catching before"
echo "    anybody else sees it."
