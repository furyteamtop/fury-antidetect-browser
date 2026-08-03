#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Every @@ header's line counts must match the hunk that follows it.

A header that disagrees with its body applies as garbage or not at all, and git
says only "corrupt patch at line N" — which names the line it gave up on, not
the header that lied. This has happened here twice, both times from hand-editing
a patch to reorder the series, and both times it cost a build cycle to find.
"""
import pathlib
import re
import sys

HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def main():
    bad = 0
    for f in sorted(pathlib.Path("core/patches").glob("*.patch")):
        lines = f.read_text().splitlines()
        i = 0
        while i < len(lines):
            m = HUNK.match(lines[i])
            if not m:
                i += 1
                continue
            want_old = int(m.group(2) or 1)
            want_new = int(m.group(4) or 1)
            old = new = 0
            j = i + 1
            while j < len(lines) and not lines[j].startswith(("@@", "diff --git")):
                c = lines[j][:1]
                if c == "+":
                    new += 1
                elif c == "-":
                    old += 1
                elif c in (" ", ""):
                    old += 1
                    new += 1
                else:
                    break
                j += 1
            if (old, new) != (want_old, want_new):
                print(
                    f"!! {f.name}:{i + 1} header says {want_old}/{want_new}, "
                    f"body has {old}/{new}",
                    file=sys.stderr,
                )
                bad += 1
            i = j
    if bad:
        print(f"\n{bad} bad hunk header(s).", file=sys.stderr)
        return 1
    print("every hunk header matches its body")
    return 0


if __name__ == "__main__":
    sys.exit(main())
