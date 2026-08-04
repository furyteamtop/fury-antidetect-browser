#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

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

  # Files named on the command line are ADDED to that list, not ignored.
  #
  # They used to be ignored, silently: the branch above won whenever the patch
  # existed, so `refresh.sh 0021 a.cc b.cc` rewrote the patch from its old file
  # list and dropped the two new files without a word. The patch came out
  # smaller than the work that went into it, the script said "Wrote ... 70
  # lines", and the only sign was noticing the number was too small.
  #
  # That is how a patch grows a second file — 0021 needed the Windows themes
  # beside the macOS one — and it has to work, or the answer is "delete the
  # patch and start over", which loses the header nobody wants to retype.
  if [ $# -gt 1 ]; then
    shift
    for extra in "$@"; do
      already=0
      for have in "${files[@]}"; do
        [ "$have" = "$extra" ] && already=1 && break
      done
      if [ "$already" = 0 ]; then
        files+=("$extra")
        echo "==> Adding $extra to $NAME"
      fi
    done
  fi
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

# A file this patch CREATES has to be intent-to-added first, or git diff does
# not see it and the refresh silently drops it.
#
# This is not hypothetical. 0110 creates five files under
# components/fury/key_provider/; they were untracked, `git diff -- <paths>`
# reported nothing for them, and the refresh rewrote the patch from 367 lines to
# 49 — losing fury_key.{h,cc}, fury_key_provider.{h,cc} and a BUILD.gn — while
# printing "==> Wrote ... 49 lines" and exiting 0. The sibling directory
# components/fury/ happened to be `git add -N`ed at some point in the past, so
# 0001 refreshed correctly on the same run, which is exactly why it took a
# by-hand comparison to notice.
for f in "${files[@]}"; do
  if [ -e "$SRC/$f" ]; then
    git -C "$SRC" add -N -- "$f" 2>/dev/null || true
  fi
done

# Written to a temporary file and moved into place only once every check below
# has passed. The first version of this wrote $OUT directly and then validated
# it, which meant a failed check left the damaged patch on disk and an error
# message that had to say "check git diff before continuing" — telling somebody
# to clean up after a tool is not the same as a tool that does not make the
# mess.
TMP="$(mktemp "${TMPDIR:-/tmp}/fury-refresh.XXXXXX")"
trap 'rm -f "$TMP"' EXIT

git -C "$SRC" diff --no-color --src-prefix=a/ --dst-prefix=b/ -- "${files[@]}" > "$TMP"

if [ ! -s "$TMP" ]; then
  echo "!! Diff is empty. Either nothing changed, or the file list is wrong." >&2
  exit 1
fi

# Every file this patch owns must appear in what was just written.
#
# The add -N above is the fix; this is the check that the fix worked, and it
# catches the other ways the list and the output can disagree — a path renamed
# upstream, a file whose change was reverted in the tree, a typo in an argument.
# Any of them produce a patch that is smaller than the work it is supposed to
# carry, and "smaller than expected" is not something a person notices in a line
# count.
missing=""
for f in "${files[@]}"; do
  grep -qxF "+++ b/$f" "$TMP" || missing="$missing $f"
done
if [ -n "$missing" ]; then
  echo "!! $NAME owns files that are not in the refreshed patch:" >&2
  for f in $missing; do echo "     $f" >&2; done
  echo "!! Refusing to write a patch smaller than the change it describes." >&2
  echo "!! $OUT is untouched." >&2
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
if ! python3 - "$TMP" "$PATCHES" "$NAME" <<'CHECK'
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
    print("!! The bad diff is at the path above; the patch itself is untouched.",
          file=sys.stderr)
    sys.exit(1)
CHECK
then
  # Kept, not deleted: the clash is fixed by splicing the correct hunk out of
  # this diff, and deleting it would mean regenerating it by hand.
  KEPT="$OUT.rejected"
  cp "$TMP" "$KEPT"
  echo "!! The diff that was refused is in $KEPT" >&2
  exit 1
fi

# Every check has passed. Only now does the patch on disk change.
mv "$TMP" "$OUT"
trap - EXIT

echo "==> Wrote $OUT ($(wc -l < "$OUT") lines)"
