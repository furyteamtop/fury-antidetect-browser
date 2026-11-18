#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""The donation address is the same everywhere it is written.

    ./check-wallet.py

The address lives in the application (desktop/src/donate.ts), both READMEs and
the site. The usual attack on a donation address in a public repository is not
a break-in: it is one changed character in a pull request, or a fork that
republishes the project with its own address. Equality across the files
catches the first; the reference below, which a pull request would have to
change in plain sight in a CI file, catches a change made everywhere at once.

Any TRON-shaped string (T followed by 33 base58 characters) in the files below
must be the reference, and each file must carry it, so a file that silently
lost the address fails as well.
"""

import re
import sys
from pathlib import Path

REFERENCE = "TNvKC6tXKtsxA4tDrjU86zbmwMGuxfmatq"
FILES = ["desktop/src/donate.ts", "README.md", "README.ru.md", "site/index.html"]
TRON = re.compile(r"(?<![1-9A-HJ-NP-Za-km-z])T[1-9A-HJ-NP-Za-km-z]{33}(?![1-9A-HJ-NP-Za-km-z])")


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    bad = []
    for name in FILES:
        text = (root / name).read_text(encoding="utf-8")
        found = TRON.findall(text)
        if REFERENCE not in found:
            bad.append(f"{name}: the donation address is missing")
        bad += [f"{name}: {a} is not the donation address" for a in sorted(set(found) - {REFERENCE})]
    for line in bad:
        print(line, file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
