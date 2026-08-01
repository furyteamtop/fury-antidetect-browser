#!/usr/bin/env bash
# Apply the patch series to the Chromium tree.
#
# Usage:
#   core/build/apply.sh                   apply all
#   core/build/apply.sh --check           dry run, report which patches would fail
#   core/build/apply.sh --allow-missing   proceed even if a listed patch is absent
#
# On the first failure it stops and tells you exactly which patch to fix.
# That is the point of a series over a merge branch.
set -euo pipefail

CORE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$CORE_DIR/src"
PATCHES="$CORE_DIR/patches"
CHECK_ONLY=0
ALLOW_MISSING=0
for arg in "$@"; do
  case "$arg" in
    --check) CHECK_ONLY=1 ;;
    --allow-missing) ALLOW_MISSING=1 ;;
    *) echo "!! unknown argument: $arg" >&2; exit 2 ;;
  esac
done

[ -d "$SRC" ] || { echo "!! No source tree. Run fetch.sh first." >&2; exit 1; }

# Read the series file: strip comments, blank lines and the '!' hot-file marker.
# while-read, not mapfile: macOS ships bash 3.2 and has no mapfile/readarray.
#
# Order matters and used not to be right. Stripping '!' before the trailing
# whitespace left a name like "0001-fp-config-plumbing.patch!" for every entry
# that carried both a marker and a comment, no such file existed, and those five
# — 0001 among them, the one every other patch depends on — were reported as
# "not written yet, skipping" before the script announced success.
series=()
while IFS= read -r line; do
  [ -n "$line" ] && series+=("$line")
done < <(
  sed -e 's/#.*$//' -e 's/[[:space:]]*$//' -e 's/!$//' "$PATCHES/series" \
    | grep -v '^$'
)

echo "==> ${#series[@]} patches in series"

failed=()
missing=()
for patch in "${series[@]}"; do
  file="$PATCHES/$patch"
  if [ ! -f "$file" ]; then
    echo "   -- $patch LISTED BUT NOT WRITTEN"
    missing+=("$patch")
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

# A patch named in the series and not on disk is a vector the built browser does
# not have. Skipping it and then reporting success is how a build shipped
# without branding, without the de-Googling, and without the DevTools lock while
# this script said the series was clean.
#
# The series file already has a convention for work that is planned and not
# written: comment the line out, with the reason. An uncommented name is a
# promise, so a broken one stops the build. --allow-missing exists for the one
# case that is not a mistake — checking whether the patches that DO exist still
# apply after a Chromium bump.
if [ ${#missing[@]} -gt 0 ]; then
  echo
  echo "==> ${#missing[@]} patches are listed in the series and do not exist:"
  for patch in "${missing[@]}"; do echo "      $patch"; done
  echo
  echo "The browser this produces is missing those vectors. Either write them,"
  echo "or comment the lines out in $PATCHES/series with the reason — the file"
  echo "already does that for 0013, 0081, 0082 and 0100."
  if [ "$ALLOW_MISSING" -eq 0 ]; then
    exit 1
  fi
  echo "Continuing anyway: --allow-missing was passed."
fi

echo "==> Series applied cleanly (${#series[@]} patches, ${#missing[@]} missing)"
