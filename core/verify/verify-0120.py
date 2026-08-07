#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""performance.memory, and the three numbers that have to make sense together.

`jsHeapSizeLimit` is a device class in one integer: a phone, a 4 GB laptop and
a 32 GB workstation report different limits, and a profile claiming 64 cores
and 32 GB of RAM while reporting a 512 MB heap has contradicted itself.

The value alone is one comparison. What this checks past it:

  * the ORDERING. used <= total <= limit, always. A patched limit that drops
    below the heap actually in use is a state no real browser reaches.
  * it is the CHROME-SHAPED quantity. Chrome rounds these to a bucket rather
    than reporting the byte count, so the answer must be exactly what was asked
    for and not a value with entropy of its own.
  * `performance.memory` is a NON-STANDARD Chrome extension, so its absence is
    itself a fingerprint. The property has to still be there, and still be an
    own property of a `MemoryInfo`, not a plain object bolted on.

Usage: core/verify/verify-0120.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# 4 GB — one of the buckets a real Chrome reports, and not what this machine
# reports on its own.
LIMIT = 4294705152

READ = """
(() => {
  const m = performance.memory;
  return JSON.stringify({
    present: !!m,
    // constructor.name is "Object" here even on stock Chromium — MemoryInfo
    // has no global constructor to name. Object.prototype.toString is the tag
    // that actually distinguishes it.
    tag: m ? Object.prototype.toString.call(m) : null,
    limit: m ? m.jsHeapSizeLimit : null,
    total: m ? m.totalJSHeapSize : null,
    used: m ? m.usedJSHeapSize : null,
    // Chrome keeps these on the prototype of MemoryInfo, not as own data
    // properties — a bolted-on object is a different shape.
    own: m ? Object.keys(m).length : null,
  });
})()
"""


def main():
    claims = Claims("0120 — performance.memory", CORE)

    with launch(CORE, {"engine": {"jsHeapSizeLimit": LIMIT}}) as s:
        got = json.loads(s.js(READ))
        print(f"  measured: {got}")

        claims.check(got["present"],
                     "performance.memory exists — its absence is a fingerprint "
                     "of its own, since only Chrome has it")
        claims.check(got["tag"] == "[object MemoryInfo]",
                     f"and it is a MemoryInfo, not a plain object bolted on "
                     f"(got {got['tag']!r})")
        claims.check(got["own"] == 0,
                     f"whose values live on the prototype, where Chrome keeps "
                     f"them (got {got['own']} own keys)")

        claims.check(got["limit"] == LIMIT,
                     f"jsHeapSizeLimit is {LIMIT} (got {got['limit']})")
        claims.check(got["used"] <= got["total"] <= got["limit"],
                     f"used <= total <= limit — a limit below the heap in use is "
                     f"a state no real browser reaches "
                     f"({got['used']} <= {got['total']} <= {got['limit']})")
        claims.check(got["used"] > 0 and got["total"] > 0,
                     f"and the other two are real measurements, not zeroes "
                     f"({got['used']}, {got['total']})")

    with launch(CORE, None) as s:
        bare = json.loads(s.js(READ))
        print(f"  unconfigured: limit {bare['limit']}")
        claims.control(bare["limit"] != LIMIT and bare["present"],
                       f"an unconfigured build reports the host's own limit "
                       f"(got {bare['limit']})")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
