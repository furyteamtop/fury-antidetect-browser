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
avail_gb=$(df -Pk "$CORE_DIR" | awk 'NR==2 {print int($4/1048576)}')
if [ "$avail_gb" -lt 150 ]; then
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
git -C "$SRC" checkout -q --detach FETCH_HEAD

echo "==> gclient sync (this is the slow part)"
(cd "$CORE_DIR" && "$GCLIENT" sync --with_branch_heads --with_tags -D --no-history)

# Record what we are pinned to, so apply.sh and CI agree.
echo "$CHROMIUM_VERSION" > "$CORE_DIR/CHROMIUM_VERSION"

echo "==> Done. Next: core/build/apply.sh"
