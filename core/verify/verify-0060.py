#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""enumerateDevices(), and the count that is really being hidden.

Before any permission is granted, `enumerateDevices()` returns entries with
empty labels and empty deviceIds — but the COUNT is still there, and the count
is the fingerprint: two cameras, three microphones and one speaker is a
narrower description of a machine than most people expect.

0060 narrows the count, and narrowing is the only honest direction. Dropping a
camera is truthful in its consequences: `getUserMedia` then fails exactly as it
does on a machine without one. Advertising a camera that is not there would
fail at capture time in a way no real machine fails — worse than the count it
was meant to hide.

Three claims past the counts:

  * the kinds are counted SEPARATELY. A single limit applied to the whole list
    would hide a microphone to satisfy a camera count.
  * the surviving entries are untouched — same kind, same shape, empty labels
    exactly as before, because a labelled device before permission is a
    louder signal than any count.
  * a limit HIGHER than the machine has does not invent anything.

Usage: core/verify/verify-0060.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

READ = """
(async () => {
  const list = await navigator.mediaDevices.enumerateDevices();
  const out = {audioinput: 0, audiooutput: 0, videoinput: 0};
  for (const d of list) { out[d.kind] = (out[d.kind] || 0) + 1; }
  return JSON.stringify({
    counts: out,
    total: list.length,
    labelled: list.filter(d => d.label !== '').length,
    kinds: list.map(d => d.kind),
  });
})()
"""


def main():
    claims = Claims("0060 — media device counts", CORE)

    with launch(CORE, None) as s:
        bare = json.loads(s.js(READ))
        print(f"  unconfigured: {bare['counts']}")

        if bare["counts"]["audioinput"] < 1 or bare["counts"]["audiooutput"] < 1:
            print(f"\nFAIL — this host reports {bare['counts']}, too few devices "
                  f"to narrow. Nothing below would mean anything.")
            return 1

        claims.control(
            sum(bare["counts"].values()) > 1 and bare["labelled"] == 0,
            f"an unconfigured build reports the machine's own {bare['counts']} "
            f"with no labels before permission — the count is what is being "
            f"hidden, and it is present to hide",
        )

    # Ask for fewer of one kind and none of another.
    want = {"audioInputCount": 1, "audioOutputCount": 1, "videoInputCount": 0}
    with launch(CORE, {"mediaDevices": want}) as s:
        got = json.loads(s.js(READ))
        print(f"  limited to {want}: {got['counts']}")

        claims.check(got["counts"]["audioinput"] == 1,
                     f"one microphone (got {got['counts']['audioinput']})")
        claims.check(got["counts"]["audiooutput"] == 1,
                     f"one speaker (got {got['counts']['audiooutput']})")
        claims.check(got["counts"]["videoinput"] == 0,
                     f"and no camera at all — getUserMedia will now fail exactly "
                     f"as it does on a machine without one "
                     f"(got {got['counts']['videoinput']})")
        claims.check(got["labelled"] == 0,
                     f"still no labels before permission — a labelled device "
                     f"here is louder than any count "
                     f"(got {got['labelled']} labelled)")

        # The kinds are counted apart from each other.
        claims.check(
            got["counts"]["audioinput"] <= bare["counts"]["audioinput"]
            and got["counts"]["audiooutput"] <= bare["counts"]["audiooutput"],
            "each kind was counted separately — one limit over the whole list "
            "would hide a microphone to satisfy a camera count",
        )

    # A limit above what the machine has must invent nothing.
    generous = {"audioInputCount": 99, "audioOutputCount": 99,
                "videoInputCount": 99}
    with launch(CORE, {"mediaDevices": generous}) as s:
        more = json.loads(s.js(READ))
        print(f"  limited to {generous}: {more['counts']}")
        claims.check(more["counts"] == bare["counts"],
                     f"a limit higher than the machine has invents nothing — "
                     f"narrowing is the only direction (got {more['counts']}, "
                     f"machine has {bare['counts']})")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
