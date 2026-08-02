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
PATCHES="$CORE_DIR/patches"
OUT="$PATCHES/$NAME.patch"

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

# A file that an EARLIER patch creates must not be re-diffed against HEAD.
#
# `git diff` compares the working tree with HEAD, where a file created by patch
# 0001 does not exist — so the diff comes out as the whole file, "new file mode"
# and all. Written to a later patch, that patch now creates a file its
# predecessor already created, the series stops applying on a clean tree, and
# nothing here says so: the diff is not empty, the patch looks plausible, and the
# breakage surfaces at the next rebase.
#
# This has happened twice. 0031 once carried 0001's whole BUILD.gn and DEPS, and
# 0302 twice carried 0001's fury_switches.{h,cc} — the second time about four
# minutes after the first was fixed by hand, because the tool had not been.
#
# So: refuse, and say which patch owns the file. The fix is to diff against the
# state after that patch rather than against HEAD, which core/verify and the
# commit history show how to do.
# Under `set -e` the check has to be a script that cannot fail on a grep miss,
# which is why this is python and not a pipeline of greps: the first attempt was
# a grep|xargs|grep chain, and pipefail killed refresh.sh silently on the very
# case it was written to catch.
if ! python3 - "$OUT" "$PATCHES" "$NAME" <<'CHECK'
import pathlib, re, sys

out, patches, name = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3]
text = out.read_text()

# Files this refresh would create outright.
created = set()
for chunk in text.split("diff --git ")[1:]:
    if chunk.splitlines()[1:2] and chunk.splitlines()[1].startswith("new file mode"):
        m = re.search(r"^\+\+\+ b/(.+)$", chunk, re.M)
        if m:
            created.add(m.group(1))

clashes = []
for other in sorted(patches.glob("*.patch")):
    if other.stem == name:
        continue
    o = other.read_text()
    for chunk in o.split("diff --git ")[1:]:
        lines = chunk.splitlines()
        if len(lines) > 1 and lines[1].startswith("new file mode"):
            m = re.search(r"^\+\+\+ b/(.+)$", chunk, re.M)
            if m and m.group(1) in created:
                clashes.append((m.group(1), other.name))

if clashes:
    print(f"!! {name} would re-create files another patch already creates:", file=sys.stderr)
    for f, owner in clashes:
        print(f"     {f}  is created by {owner}", file=sys.stderr)
    print("!! Diff those against the tree AFTER that patch, not against HEAD.", file=sys.stderr)
    print("!! The bad diff is left in place so the correct hunk can be spliced in.",
          file=sys.stderr)
    sys.exit(1)
CHECK
then
  exit 1
fi

echo "==> Wrote $OUT ($(wc -l < "$OUT") lines)"
