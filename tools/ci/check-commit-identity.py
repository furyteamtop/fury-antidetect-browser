#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""No commit may carry a machine-derived identity.

    ./check-commit-identity.py

When git has no `user.email` configured it does not stop. It builds one from the
account name and the hostname — `<login>@<Their-Laptop>.local` — and commits with
it, silently. That address goes into the commit object, which means it goes into
every clone, and it names both the person's login and their machine.

(The real one is not written here. Quoting it would publish exactly the two
strings this check exists to keep out of a public history, which would be a
strange way to make the point.)

It happened here, twice, in the two commits immediately before publication.

The cause is worth stating because it is not carelessness, it is a gap: this
repository's identity was set in `.git/config`, which is LOCAL and therefore not
part of the history, not part of a clone, and not restored by anything. During a
history rewrite the `.git` directory was replaced wholesale, and with it the
only place that identity lived. The next two commits fell back to the machine's
name. Nothing warned, because from git's point of view nothing had gone wrong.

The blob sweep run before publication did not catch it either, and could not:
it searched the contents of every object that has ever existed, and an author
address is not in a blob. It is in the commit.

So: a check that reads commits rather than files.

It runs over the whole history because that is cheap at this size and because
the failure it looks for is one-commit-deep — a single bad commit among a
hundred good ones is exactly the case a spot check misses.
"""

import re
import subprocess
import sys

# What the project commits as. A GitHub noreply address, deliberately: the
# author does not want a personal email address in a public history, and once
# published there is no taking it back.
EXPECTED = "furyteamtop@users.noreply.github.com"

# The shapes a machine-derived address takes. `.local` is what macOS appends to
# a hostname; `.lan`, `.home` and `.internal` are the common router defaults;
# `(none)` is what Linux uses when it has no domain at all.
MACHINE = re.compile(
    r"@.*\.(local|lan|home|internal|localdomain)$|@\(none\)$|@localhost$",
    re.I,
)


def main() -> int:
    log = subprocess.run(
        ["git", "log", "--format=%H%x00%an%x00%ae%x00%cn%x00%ce"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.splitlines()

    bad = []
    for line in log:
        if not line:
            continue
        sha, an, ae, cn, ce = line.split("\0")
        for role, name, email in (("author", an, ae), ("committer", cn, ce)):
            if email != EXPECTED:
                why = (
                    "machine-derived — this names a login and a hostname"
                    if MACHINE.search(email)
                    else "not the project identity"
                )
                bad.append((sha[:12], role, f"{name} <{email}>", why))

    if bad:
        print(
            f"!! {len(bad)} commit identit(ies) are not {EXPECTED}:", file=sys.stderr
        )
        for sha, role, who, why in bad:
            print(f"     {sha}  {role:9} {who}", file=sys.stderr)
            print(f"                          {why}", file=sys.stderr)
        print(
            "\n!! Set it, so it cannot depend on whatever is configured globally:\n"
            f'!!     git config --local user.name "Bogdan Shapovalov"\n'
            f'!!     git config --local user.email "{EXPECTED}"\n'
            "!!\n"
            "!! Then rewrite the offending commits — while they are still local,\n"
            "!! which is the only time it is free:\n"
            "!!     git filter-branch -f --env-filter '...' <range>\n"
            "!!\n"
            "!! .git/config is not part of the repository, so nothing restores it\n"
            "!! after a clone or a history rewrite. That is how this happened.",
            file=sys.stderr,
        )
        return 1

    print(f"all {len(log)} commits are authored and committed by {EXPECTED}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
