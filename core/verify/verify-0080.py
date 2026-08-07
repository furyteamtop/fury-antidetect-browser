#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""The timezone, in the four places a page can read it.

`Intl.DateTimeFormat().resolvedOptions().timeZone` is the obvious one and the
easy one. The three that catch a half-done patch:

  * `Date.prototype.getTimezoneOffset()` — computed by V8 from ICU's zone. A
    patch that changes the NAME and not the offset produces a browser that says
    Europe/Berlin and does UTC arithmetic, which any page can subtract.
  * the offset in WINTER and in SUMMER. A zone with daylight saving has two, and
    a spoof that pins one number reports the same offset in January and July —
    which Berlin does not.
  * a Worker. Its own ICU, its own global.

Usage: core/verify/verify-0080.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# Berlin: +60 in winter, +120 in summer, and getTimezoneOffset reports the
# NEGATIVE of that. Chosen because it has daylight saving and this machine
# does not sit in it.
ZONE = "Europe/Berlin"
WINTER_OFFSET = -60
SUMMER_OFFSET = -120

READ = """
(async () => {
  const out = {
    zone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    winter: new Date('2026-01-15T12:00:00Z').getTimezoneOffset(),
    summer: new Date('2026-07-15T12:00:00Z').getTimezoneOffset(),
    formatted: new Date('2026-01-15T12:00:00Z').toString(),
  };
  const w = new Worker(URL.createObjectURL(new Blob([
    "postMessage(JSON.stringify({" +
    "zone: Intl.DateTimeFormat().resolvedOptions().timeZone," +
    "winter: new Date('2026-01-15T12:00:00Z').getTimezoneOffset()}))"])));
  out.worker = JSON.parse(await new Promise((r) => { w.onmessage = (e) => r(e.data); }));
  w.terminate();
  return JSON.stringify(out);
})()
"""


def main():
    claims = Claims("0080 — timezone", CORE)

    with launch(CORE, {"locale": {"timezone": ZONE}}) as s:
        got = json.loads(s.js(READ))
        print(f"  measured: {got}")

        claims.check(got["zone"] == ZONE,
                     f"Intl reports {ZONE} (got {got['zone']!r})")

        # The one a name-only patch fails.
        claims.check(got["winter"] == WINTER_OFFSET,
                     f"getTimezoneOffset in January is {WINTER_OFFSET} — a patch that "
                     f"changes the name and not the arithmetic fails here "
                     f"(got {got['winter']})")
        claims.check(got["summer"] == SUMMER_OFFSET,
                     f"and {SUMMER_OFFSET} in July (got {got['summer']})")
        claims.check(got["winter"] != got["summer"],
                     "the two differ, because Berlin has daylight saving and a "
                     "pinned number would not")

        claims.check("GMT+0100" in got["formatted"] or "+0100" in got["formatted"],
                     f"Date.toString carries the same offset (got {got['formatted']!r})")

        claims.check(got["worker"]["zone"] == ZONE
                     and got["worker"]["winter"] == WINTER_OFFSET,
                     f"a Worker agrees, name and arithmetic both: {got['worker']}")

    with launch(CORE, None) as s:
        bare = s.js("Intl.DateTimeFormat().resolvedOptions().timeZone")
        print(f"  unconfigured: {bare}")
        claims.control(bare != ZONE,
                       f"an unconfigured build reports the host's zone, not {ZONE} "
                       f"(got {bare!r})")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
