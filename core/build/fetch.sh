#!/usr/bin/env bash
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
avail_gb=$(df -Pg "$CORE_DIR" | awk 'NR==2 {print $4}')
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
(cd "$CORE_DIR" && gclient sync --with_branch_heads --with_tags -D --no-history)

# Record what we are pinned to, so apply.sh and CI agree.
echo "$CHROMIUM_VERSION" > "$CORE_DIR/CHROMIUM_VERSION"

echo "==> Done. Next: core/build/apply.sh"
