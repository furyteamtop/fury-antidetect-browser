#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Every verify script, and a table at the end.

The CI job used to be a shell loop under `set -e`, which stops at the first
failure. That is the wrong shape for this: when a Chromium uprev breaks four
patches, one failure and twenty-six unknowns is a worse report than four
failures and twenty-two passes, and it takes four builds to learn what one
would have told you.

So every script runs, whatever the ones before it did, and the exit code is
non-zero if any failed.

Usage: core/verify/run-all.py <core binary> [pattern ...]
"""

import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def main():
    if len(sys.argv) < 2:
        print(__doc__.strip())
        return 2
    core = sys.argv[1]
    patterns = sys.argv[2:]

    if not os.access(core, os.X_OK):
        print(f"not an executable core: {core}")
        return 2

    scripts = sorted(
        os.path.join(HERE, n) for n in os.listdir(HERE)
        if n.startswith("verify-") and n.endswith(".py")
        and (not patterns or any(p in n for p in patterns))
    )
    if not scripts:
        print("no verify scripts matched")
        return 2

    print(f"{len(scripts)} scripts against {core}\n")
    results = []
    for path in scripts:
        name = os.path.basename(path)
        started = time.monotonic()
        proc = subprocess.run([sys.executable, path, core],
                              capture_output=True, text=True)
        took = time.monotonic() - started
        results.append((name, proc.returncode, took, proc.stdout, proc.stderr))
        print(f"== {name}")
        print(proc.stdout.rstrip())
        if proc.returncode != 0 and proc.stderr.strip():
            print(proc.stderr.rstrip())
        print()

    failed = [r for r in results if r[1] != 0]
    width = max(len(r[0]) for r in results)
    print("=" * (width + 24))
    for name, code, took, _, _ in results:
        print(f"  {name.ljust(width)}  {'ok  ' if code == 0 else 'FAIL'}  "
              f"{took:6.1f}s")
    print("=" * (width + 24))

    total = sum(r[2] for r in results)
    if failed:
        print(f"\n{len(failed)} of {len(results)} failed in {total:.0f}s: "
              f"{', '.join(r[0] for r in failed)}")
        return 1
    print(f"\nall {len(results)} passed in {total:.0f}s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
