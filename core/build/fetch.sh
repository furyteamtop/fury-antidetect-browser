#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

# Fetch or update the Chromium source tree at a pinned tag.
#
# Usage: core/build/fetch.sh 151.0.7842.60
#
# Disk: expect ~100 GB after sync, ~200 GB after a build. An external drive will
# not do — the build is IOPS-bound.
set -euo pipefail

CHROMIUM_VERSION="${1:?usage: fetch.sh <chromium-tag>}"
CORE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEPOT_TOOLS="$CORE_DIR/depot_tools"
SRC="$CORE_DIR/src"

echo "==> Chromium $CHROMIUM_VERSION into $SRC"

# --- free space check -------------------------------------------------------
# -Pk and the division, rather than the -Pg that stood here until 15.08.2026.
#
# -g is a BSD flag. macOS has it, GNU coreutils does not, and Git Bash on
# Windows ships GNU — so this line printed "df: unknown option -- g" and, under
# `set -e`, took the whole script down before a single byte was fetched. Found
# by the first person to run fetch.sh on Windows, thirty seconds in.
#
# -Pk is POSIX: 1024-byte blocks, portable everywhere, same answer on both.
#
# 150 is for a first fetch. Moving an existing tree between milestones needs a
# fraction of it -- 153 -> 155 on the Windows box fetched a few GB -- so
# FURY_MIN_FREE_GB lowers the bar for that case (used 29.09.2026 on a laptop
# with 109 GB free and a 70 GB tree already in place).
avail_gb=$(df -Pk "$CORE_DIR" | awk 'NR==2 {print int($4/1048576)}')
if [ "$avail_gb" -lt "${FURY_MIN_FREE_GB:-150}" ]; then
  echo "!! Only ${avail_gb} GB free. Syncing needs ~100 GB and building ~200 GB." >&2
  echo "!! Free up space or point CORE_DIR at another volume." >&2
  exit 1
fi

# --- depot_tools ------------------------------------------------------------
if [ ! -d "$DEPOT_TOOLS" ]; then
  echo "==> Cloning depot_tools"
  git clone --depth 1 \
    https://chromium.googlesource.com/chromium/tools/depot_tools.git "$DEPOT_TOOLS"
else
  git -C "$DEPOT_TOOLS" pull --ff-only
fi
export PATH="$DEPOT_TOOLS:$PATH"
export DEPOT_TOOLS_UPDATE=0

# depot_tools ships each entry point twice: a POSIX script with no extension and
# a .bat beside it. Under Git Bash the extensionless one wins, and it is the
# wrong one — it takes the POSIX path through a toolchain that only the .bat
# half sets up. Choose explicitly rather than letting PATH resolution decide.
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) GCLIENT="gclient.bat" ;;
  *)                    GCLIENT="gclient"     ;;
esac

# --- one-time depot_tools bootstrap -----------------------------------------
# gn and friends need python3_bin_reldir.txt, which only appears after a
# bootstrap. DEPOT_TOOLS_UPDATE=0 (kept, for reproducible builds) suppresses the
# implicit one, so run it explicitly — and run it with cwd INSIDE depot_tools,
# because its scripts resolve relative paths from the working directory, not
# from their own location. Invoking it as ./depot_tools/ensure_bootstrap fails
# with a confusing "cipd_client_version.digests: No such file" instead.
#
# On Windows this is a different program, and running the POSIX one there is not
# a degraded bootstrap but no bootstrap at all. Measured 15.08.2026 on the first
# Windows run: `./ensure_bootstrap` printed "Python was not found; run without
# arguments to install from the Microsoft Store" — the App Execution Alias stub
# answering, because the POSIX path expects a system python that Windows does
# not have — and then returned 0, so `set -e` let the script continue as though
# it had worked.
#
# It had not. The Windows bootstrap lives in bootstrap/win_tools.bat and is
# triggered by running gclient.bat once; among other things it CREATES git.bat,
# which is not in the depot_tools repository. gclient sync then died with a bare
# FileNotFoundError out of git_cache.py:218, where `git_exe` is 'git.bat' on
# Windows and the surrounding except clause catches only CalledProcessError.
#
# Nothing in that traceback names the bootstrap.
if [ ! -f "$DEPOT_TOOLS/python3_bin_reldir.txt" ]; then
  echo "==> Bootstrapping depot_tools (one time)"
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) (cd "$DEPOT_TOOLS" && cmd //c gclient.bat >/dev/null) ;;
    *)                    (cd "$DEPOT_TOOLS" && ./ensure_bootstrap)             ;;
  esac
fi

# --- initial fetch ----------------------------------------------------------
if [ ! -d "$SRC" ]; then
  echo "==> First fetch. This takes a while (tens of GB)."
  mkdir -p "$SRC"
  cat > "$CORE_DIR/.gclient" <<EOF
solutions = [
  {
    "name": "src",
    "url": "https://chromium.googlesource.com/chromium/src.git",
    "managed": False,
    "custom_deps": {},
    "custom_vars": {
      "checkout_pgo_profiles": True,
    },
  },
]
EOF
  git -C "$SRC" init -q
  git -C "$SRC" remote add origin https://chromium.googlesource.com/chromium/src.git
fi

# --- checkout the tag -------------------------------------------------------
echo "==> Fetching tag $CHROMIUM_VERSION"
git -C "$SRC" fetch --depth 1 origin "refs/tags/$CHROMIUM_VERSION"

# A tree with the patch series on it cannot be checked out over, and silently
# discarding somebody's edits is not this script's call to make. The series is
# regenerable from core/patches and the icons from core/build/link-icons.sh, so
# the fix is one line -- but it is the caller who runs it.
# --ignore-submodules=all and no untracked files, because neither is evidence of
# anybody's work. Chromium tracks its dependencies as submodules and gclient
# moves those gitlinks to the DEPS-pinned revisions, so a HEALTHY tree that has
# just synced shows forty-odd modified entries; untracked content under
# third_party/ is what the sync materialises. Counting either would make this
# refuse to run a second time on a tree it produced itself -- measured
# 10.09.2026, resuming a sync that a 429 had interrupted.
#
# What is left is what it means to ask about: an edited source file, which is
# either the applied patch series or somebody's work in progress.
if [ -n "$(git -C "$SRC" status --porcelain --untracked-files=no --ignore-submodules=all)" ]; then
  echo "!! $SRC has local changes. If they are only the applied patch series," >&2
  echo "!! and core/build/apply.sh can put them back, clear it with:" >&2
  echo >&2
  echo "     git -C $SRC reset --hard && git -C $SRC clean -fd -e out" >&2
  echo "     git -C $SRC/third_party/webrtc reset --hard    # 0071 patches this checkout" >&2
  echo >&2
  echo "!! Anything else in there is yours and this will not touch it." >&2
  exit 1
fi

# `-f`, and a loop that removes what git names, because moving between
# milestones is not a plain checkout. Upstream starts TRACKING files that the
# previous milestone's DEPS pulled in as untracked gclient content, and git
# refuses to overwrite an untracked file rather than choosing for you:
#
#   error: The following untracked working tree files would be overwritten
#          by checkout: third_party/aria-practices/src/LICENSE.md ...
#   Aborting
#
# Measured 09.09.2026 going 150 -> 153. Under `set -e` fetch.sh died there,
# before gclient sync, with a message about files and nothing about versions.
#
# Only paths under third_party/ are ever deleted: those are gclient's, and the
# sync below puts them back. A blocker anywhere else stops this, because it
# would mean something other than a dependency has moved.
for attempt in 1 2 3 4 5 6 7 8 9 10; do
  if err=$(git -C "$SRC" checkout -f -q --detach FETCH_HEAD 2>&1); then
    break
  fi
  blockers=$(printf '%s\n' "$err" | sed -n 's/^\t\(third_party\/[^/]*\/[^/]*\).*/\1/p' | sort -u)
  if [ -z "$blockers" ]; then
    echo "!! Cannot check out $CHROMIUM_VERSION:" >&2
    printf '%s\n' "$err" >&2
    exit 1
  fi
  echo "==> clearing $(printf '%s\n' "$blockers" | wc -l | tr -d ' ') dependency dir(s) upstream now tracks"
  printf '%s\n' "$blockers" | while read -r d; do [ -n "$d" ] && rm -rf "$SRC/$d"; done
  [ "$attempt" = 10 ] && { echo "!! still blocked after 10 rounds" >&2; exit 1; }
done

echo "==> gclient sync (this is the slow part)"

# Retried, because the common failure here is not ours and not permanent.
# chromium.googlesource.com rate-limits anonymous fetches, and a sync that pulls
# a few hundred dependencies trips it:
#
#   remote: RESOURCE_EXHAUSTED ... "Short term server-time rate limit exceeded"
#   fatal: ... libphonenumber.git ...: The requested URL returned error: 429
#
# Measured 10.09.2026 on the macOS 153 sync, fourteen minutes in. gclient sync
# is resumable -- what it already has it keeps -- so the answer is to wait and
# ask again rather than to start over. Backs off 60s, 120s, 240s, 480s.
sync_attempt=1
delay=60
until (cd "$CORE_DIR" && "$GCLIENT" sync --with_branch_heads --with_tags -D --no-history); do
  if [ "$sync_attempt" -ge 5 ]; then
    echo "!! gclient sync failed 5 times. The last error is above; if it is a 429" >&2
    echo "!! the limit is per-hour and waiting longer is the fix, not a flag." >&2
    exit 1
  fi
  echo "==> sync attempt $sync_attempt failed; waiting ${delay}s and resuming"
  sleep "$delay"
  sync_attempt=$((sync_attempt + 1))
  delay=$((delay * 2))
done

# Record what we are pinned to, so apply.sh and CI agree.
echo "$CHROMIUM_VERSION" > "$CORE_DIR/CHROMIUM_VERSION"

echo "==> Done. Next: core/build/apply.sh"
