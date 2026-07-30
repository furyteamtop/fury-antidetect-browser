#!/usr/bin/env bash
# Apply the patch series to the Chromium tree.
#
# Usage:
#   core/build/apply.sh            apply all
#   core/build/apply.sh --check    dry run, report which patches would fail
#
# On the first failure it stops and tells you exactly which patch to fix.
# That is the point of a series over a merge branch.
set -euo pipefail

CORE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$CORE_DIR/src"
PATCHES="$CORE_DIR/patches"
CHECK_ONLY=0
[ "${1:-}" = "--check" ] && CHECK_ONLY=1

[ -d "$SRC" ] || { echo "!! No source tree. Run fetch.sh first." >&2; exit 1; }

# Read the series file: strip comments, blank lines and the '!' hot-file marker.
mapfile -t series < <(
  sed -e 's/#.*$//' -e 's/!$//' -e 's/[[:space:]]*$//' "$PATCHES/series" \
    | grep -v '^$'
)

echo "==> ${#series[@]} patches in series"

failed=()
for patch in "${series[@]}"; do
  file="$PATCHES/$patch"
  if [ ! -f "$file" ]; then
    echo "   -- $patch (not written yet, skipping)"
    continue
  fi

  if git -C "$SRC" apply --check "$file" 2>/dev/null; then
    if [ "$CHECK_ONLY" -eq 0 ]; then
      git -C "$SRC" apply "$file"
      echo "   ok $patch"
    else
      echo "   ok $patch (dry run)"
    fi
  else
    echo "   !! $patch DOES NOT APPLY"
    failed+=("$patch")
    if [ "$CHECK_ONLY" -eq 0 ]; then
      echo
      echo "Fix it with:"
      echo "  cd $SRC"
      echo "  git apply --3way $file      # leaves conflict markers"
      echo "  # ...resolve, then:"
      echo "  $CORE_DIR/build/refresh.sh ${patch%.patch}"
      exit 1
    fi
  fi
done

if [ ${#failed[@]} -gt 0 ]; then
  echo
  echo "==> ${#failed[@]} patches need rebasing: ${failed[*]}"
  exit 1
fi

echo "==> Series applied cleanly"
