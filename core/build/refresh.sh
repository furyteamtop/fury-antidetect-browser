#!/usr/bin/env bash
# Regenerate one patch file from the current state of the source tree.
#
# Usage: core/build/refresh.sh 0031-webgl-params
#
# Workflow: edit files in core/src/, verify, then refresh the patch so the change
# is captured. The source tree is disposable; the patches are the real artefact.
set -euo pipefail

CORE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$CORE_DIR/src"
NAME="${1:?usage: refresh.sh <patch-name-without-.patch>}"
NAME="${NAME%.patch}"
OUT="$CORE_DIR/patches/$NAME.patch"

# Which files does this patch own? Taken from the existing patch when present,
# otherwise from the caller as extra arguments.
if [ -f "$OUT" ]; then
  # while-read, not mapfile: macOS bash is 3.2.
  files=()
  while IFS= read -r line; do
    [ -n "$line" ] && files+=("$line")
  done < <(grep '^+++ b/' "$OUT" | sed 's|^+++ b/||')
elif [ $# -gt 1 ]; then
  shift
  files=("$@")
else
  echo "!! $OUT does not exist yet." >&2
  echo "!! Pass the files it should own:" >&2
  echo "     refresh.sh $NAME third_party/blink/renderer/.../foo.cc" >&2
  exit 1
fi

echo "==> Refreshing $NAME from ${#files[@]} file(s)"
git -C "$SRC" diff --no-color --src-prefix=a/ --dst-prefix=b/ -- "${files[@]}" > "$OUT"

if [ ! -s "$OUT" ]; then
  echo "!! Diff is empty. Either nothing changed, or the file list is wrong." >&2
  exit 1
fi

echo "==> Wrote $OUT ($(wc -l < "$OUT") lines)"
