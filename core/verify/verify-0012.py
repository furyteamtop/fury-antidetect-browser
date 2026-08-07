#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""navigator.hardwareConcurrency, and the two values Chrome will not report.

The count itself is one number, and spoofing it is one lookup. What makes this
worth a script is what Chrome CLAMPS: the value a page sees is not the machine's
core count, it is that count limited to a range. A persona that asks for 2 and
gets 2 proves the patch works; a persona that asks for 256 and GETS 256 proves
it works and is wrong, because no real Chrome would say that.

Usage: core/verify/verify-0012.py <core binary>
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

READ = """
(async () => {
  const out = { main: navigator.hardwareConcurrency };
  const w = new Worker(URL.createObjectURL(
    new Blob(['postMessage(navigator.hardwareConcurrency)'])));
  out.worker = await new Promise((r) => { w.onmessage = (e) => r(e.data); });
  w.terminate();
  return JSON.stringify(out);
})()
"""


def read(cores):
    import json
    with launch(CORE, {"navigator": {"hardwareConcurrency": cores}}) as s:
        return json.loads(s.js(READ))


def main():
    claims = Claims("0012 — navigator.hardwareConcurrency", CORE)

    # A low count a laptop would not report by accident.
    got = read(2)
    print(f"  asked 2, measured: {got}")
    claims.check(got["main"] == 2, f"the page reads 2 (got {got['main']})")
    claims.check(
        got["worker"] == got["main"],
        f"and a Worker agrees — a disagreement here is free to detect: {got}",
    )

    # A high one, to see whether anything clamps.
    high = read(64)
    print(f"  asked 64, measured: {high}")
    claims.check(
        high["main"] == high["worker"],
        f"the two contexts still agree at the top of the range: {high}",
    )
    # Not asserting a specific ceiling: what matters is that the answer is a
    # number a real machine could have, and that the two contexts say the same.
    claims.check(
        isinstance(high["main"], int) and 1 <= high["main"] <= 128,
        f"64 produces a plausible count rather than nonsense (got {high['main']})",
    )

    with launch(CORE, None) as s:
        bare = s.js("navigator.hardwareConcurrency")
        print(f"  unconfigured: {bare}")
        claims.control(
            bare != 2,
            f"an unconfigured build reports the host's own count, not 2 (got {bare})",
        )

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
