#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Run one job across every profile, one at a time.

    FURY_API_PORT=35000 fury-agent serve
    ./rotate.py                      # dry run: says what it would do
    ./rotate.py --go                 # does it

This is the shape of the thing people actually want from an anti-detect
browser's API, and it is the shape most likely to get an account banned if it is
written carelessly. The care is in three places, all of them in this file:

  - **one at a time.** Ten browsers on ten proxies opening the same site within
    the same second is a pattern, and the pattern is what gets found — not any
    single fingerprint. Concurrency here would be faster and worse.

  - **stopped whatever happens.** A profile left running holds its lock. On a
    team server that lock is what stops a colleague opening the same account
    somewhere else, so leaking one blocks a person, not a process.

  - **it keeps going.** One profile failing must not abandon the other nine
    half-way, and the summary at the end has to say which failed rather than
    scrolling past.
"""

import argparse
import random
import sys
import time

from fury import Fury, running


def do_the_job(session: dict, profile: dict) -> str:
    """Whatever you actually want done. Replace this.

    Gets the launch response — `ws_endpoint`, `pid`, `relay_port` — and the
    profile. Attach Playwright to `session["ws_endpoint"]` (see
    playwright_example.py), or do nothing at all: opening a profile now and then
    is itself the job for accounts that must look used.

    Returns a line for the summary.
    """
    endpoint = session.get("ws_endpoint")
    return f"opened, cdp={'yes' if endpoint else 'no'}, pid={session.get('pid')}"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--go", action="store_true", help="actually do it (default is a dry run)")
    ap.add_argument("--project", help="only profiles in this project")
    ap.add_argument("--min-gap", type=float, default=20, help="seconds between profiles, minimum")
    ap.add_argument("--max-gap", type=float, default=90, help="seconds between profiles, maximum")
    args = ap.parse_args()

    fury = Fury()
    profiles = fury.profiles()

    if args.project:
        profiles = [p for p in profiles if p.get("project_id") == args.project]

    # A profile that is already open belongs to somebody — a person at the
    # keyboard, or another run of this. Taking it would put two sessions in one
    # account, which is the exact thing the lock exists to prevent.
    busy = [p for p in profiles if p.get("running")]
    profiles = [p for p in profiles if not p.get("running")]

    if busy:
        print(f"skipping {len(busy)} already open: {', '.join(p['name'] for p in busy)}")

    if not profiles:
        print("nothing to do")
        return 0

    # Shuffled, because a fixed order is itself a signal: the same accounts
    # touched in the same sequence every night is a pattern across accounts that
    # no per-profile fingerprint can hide.
    random.shuffle(profiles)

    if not args.go:
        print(f"would run over {len(profiles)} profiles, {args.min_gap:.0f}-{args.max_gap:.0f}s apart:")
        for p in profiles:
            print(f"  {p['name']}")
        print("\nadd --go to do it")
        return 0

    results, failures = [], 0
    for i, profile in enumerate(profiles):
        print(f"[{i + 1}/{len(profiles)}] {profile['name']}", flush=True)
        try:
            with running(fury, profile["id"], cdp=True) as session:
                results.append((profile["name"], do_the_job(session, profile)))
        except Exception as e:
            # Kept going deliberately. Abandoning eight profiles because the
            # second one's proxy was down is a worse outcome than eight
            # successes and a line in the summary.
            failures += 1
            results.append((profile["name"], f"FAILED: {e}"))
            print(f"    {e}", file=sys.stderr)

        if i + 1 < len(profiles):
            # An irregular gap rather than a fixed one, for the same reason as
            # the shuffle: `sleep(60)` between accounts is a metronome, and a
            # metronome is visible across accounts however good each one looks.
            gap = random.uniform(args.min_gap, args.max_gap)
            print(f"    waiting {gap:.0f}s", flush=True)
            time.sleep(gap)

    print(f"\n{len(results) - failures}/{len(results)} ok")
    for name, outcome in results:
        print(f"  {name:<28} {outcome}")

    # Non-zero when anything failed, so this can be a cron line that tells you.
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
