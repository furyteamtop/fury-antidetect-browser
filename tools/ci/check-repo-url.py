#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Every URL naming this repository must name the same one.

    ./check-repo-url.py

Eleven tracked files name it: Cargo.toml's `repository`, deploy/workspace.toml,
README.md, docs/13, docs/15 in both languages, the two issue-template links, the
shell's about box, and — the one that matters most — the auto-updater's endpoint
in desktop/src-tauri/src/update.rs.

They had drifted before the repository was ever published. Nine said one owner
and two said another — and the two were the download links on the install page,
which is the page a stranger is sent to first. Neither was wrong on its own and
nothing anywhere would have failed loudly: the install page would have 404'd for
every visitor, while the updater quietly queried a repository with no releases
in it and reported, correctly and uselessly, that there was nothing to update.

(The two names are not quoted here. This file was rewritten by a blind
search-and-replace when the repository was renamed, and quoting them turned a
historical note into a false one — the docstring ended up describing a drift
between a name and itself. A comment about a string is a comment a rename will
edit.)

That is the shape of the problem. A URL is a string, strings drift, and the
failure is silent at both ends.

The rule, and why it is this rule
---------------------------------
`Cargo.toml`'s `repository` field is the authority — it is the one a published
crate carries, so it is the one that has to be right anyway. Every other URL
whose REPOSITORY COMPONENT is this project's name must have the same owner.

That test needs no list of third parties. `vitejs/vite`, `zhom/donutbrowser` and
the five `sponsors/…` links in package-lock.json do not carry this project's
repository name, so they are not ours and are not asked about. The name is read
from Cargo.toml rather than written here, for the reason in the paragraph
above.

An allowlist was the first attempt and it was wrong twice in five minutes: it
walked the directory tree rather than the tracked files, so it read 252 URLs out
of a depot_tools CIPD manifest and several thousand out of @babel/parser's
changelog — the exclusion said "node_modules" and the path was
"desktop/node_modules". Then, given tracked files, it demanded that every GitHub
URL in the repository agree with every other one, which flagged the competitor
analysis in docs/08 for citing competitors. Both failures were the same failure:
a list somebody has to keep right. `git ls-files` and "does it match Cargo.toml"
are not lists.
"""

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]

# Both spellings: the human URL and api.github.com/repos/…
PATTERN = re.compile(
    r"github\.com/(?:repos/)?([A-Za-z0-9][A-Za-z0-9-]*)/([A-Za-z0-9._-]+)"
)


def authority() -> tuple[str, str]:
    """The owner and repo from Cargo.toml's `repository` field."""
    text = (ROOT / "Cargo.toml").read_text()
    m = re.search(r'^repository\s*=\s*"([^"]+)"', text, re.M)
    if not m:
        raise SystemExit("Cargo.toml has no `repository` field to check against")
    url = m.group(1)
    hit = PATTERN.search(url)
    if not hit:
        raise SystemExit(f"Cargo.toml's repository is not a GitHub URL: {url}")
    return hit.group(1), hit.group(2).removesuffix(".git")


def tracked():
    """Tracked files only — definitionally the set that gets published."""
    out = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.splitlines()
    for rel in out:
        path = ROOT / rel
        if path.is_file():
            yield path, rel


def main() -> int:
    owner, project = authority()

    ours: list[str] = []
    wrong: list[tuple[str, str]] = []

    for path, rel in tracked():
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        for n, line in enumerate(text.splitlines(), 1):
            for found_owner, found_repo in PATTERN.findall(line):
                if found_repo.removesuffix(".git") != project:
                    continue  # somebody else's repository, not our business
                where = f"{rel}:{n}"
                if found_owner == owner:
                    ours.append(where)
                else:
                    wrong.append((where, f"{found_owner}/{found_repo}"))

    if wrong:
        print(
            f"!! Cargo.toml says this repository is {owner}/{project}, "
            f"but {len(wrong)} reference(s) disagree:",
            file=sys.stderr,
        )
        for where, what in wrong:
            print(f"     {where}  ->  {what}", file=sys.stderr)
        print(
            "\n!! The updater endpoint in desktop/src-tauri/src/update.rs and the\n"
            "!! download links in docs/15 must resolve to the same place, or one\n"
            "!! of them breaks at the first release and neither reports an error.\n"
            "!! Changing where this is published means changing all of them.",
            file=sys.stderr,
        )
        return 1

    print(f"all {len(ours) + 1} references name {owner}/{project}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
