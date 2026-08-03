#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""No two patches may create the same file.

refresh.sh diffs against HEAD, where a file created by patch 0001 does not
exist — so refreshing a LATER patch that also touches that file rewrites it as
"new file mode" with the whole file inside. The series then stops applying on a
clean tree, and nothing complains: the diff is non-empty and the patch looks
plausible in review.

0031 once carried 0001's BUILD.gn and DEPS this way, and 0302 carried 0001's
fury_switches.{h,cc} twice in one evening — the second time four minutes after
the first was fixed by hand, because the tool had not been. refresh.sh refuses
it now; this is the check that the refusal was not worked around.
"""
import collections
import pathlib
import re
import sys


def creations(text):
    for chunk in text.split("diff --git ")[1:]:
        lines = chunk.splitlines()
        if len(lines) > 1 and lines[1].startswith("new file mode"):
            m = re.search(r"^\+\+\+ b/(.+)$", chunk, re.M)
            if m:
                yield m.group(1)


def main():
    by_file = collections.defaultdict(list)
    for f in sorted(pathlib.Path("core/patches").glob("*.patch")):
        for created in creations(f.read_text()):
            by_file[created].append(f.name)

    clashes = {k: v for k, v in by_file.items() if len(v) > 1}
    for path, patches in clashes.items():
        print(f"!! {path} is created by {', '.join(patches)}", file=sys.stderr)
    if clashes:
        print(
            "\nOnly one patch may create a file. A later patch that touches it "
            "must carry an incremental hunk — diff against the tree AFTER the "
            "patch that creates it, not against HEAD.",
            file=sys.stderr,
        )
        return 1
    print(f"{len(by_file)} file(s) created, each by exactly one patch")
    return 0


if __name__ == "__main__":
    sys.exit(main())
