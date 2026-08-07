#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""The installed font list, narrowed — and narrowed HONESTLY.

There is no API that lists fonts. Every fingerprinting library probes instead:
render a string in the family under test with a known fallback behind it, and
compare its width against the same string in a family that certainly does not
exist. Different width means the font resolved. Repeat for a few hundred
families and the set that resolved is the fingerprint.

So this script probes exactly that way, because a claim about `document.fonts`
would be a claim about an API nobody uses for this.

0050 narrows only — it can hide a font the machine has, never invent one it
does not. That asymmetry is the point: a hidden font simply fails to resolve,
which is what happens on a machine without it. Advertising a font that is not
installed would fail at RENDER time, in a way no real machine fails, which is
worse than the list it was meant to hide.

Four claims:

  * a family in the list still resolves.
  * a family the machine HAS but the list omits no longer resolves — the whole
    point, and the one that fails if the filter is not reached.
  * a family that does not exist anywhere still does not resolve. A filter that
    accidentally made everything resolve would pass the first claim alone.
  * generic families still render, AND still differ from each other. This is
    the claim the script was written for and the one that failed: `monospace`
    reaches the filter already substituted for the platform's fixed font, so
    filtering that name sent every generic to the last-resort face and made
    serif, sans-serif, monospace and cursive all measure 1044.53 px. A page
    that compares two of them and finds them equal has found a machine that
    does not exist.

Usage: core/verify/verify-0050.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# Present on every macOS since forever. The list allows the first and omits the
# second, so the second is the one that proves the filter runs.
ALLOWED = "Georgia"
WITHHELD = "Papyrus"
NOWHERE = "Fury Nonexistent Family 9182"

FONTS = ["Georgia", "Arial", "Times New Roman", "Courier New", "Verdana"]

# The width comparison every library uses.
PROBE = """
(() => {
  const measure = (family) => {
    const s = document.createElement('span');
    s.style.cssText = 'position:absolute;left:-9999px;font-size:72px;white-space:nowrap';
    s.style.fontFamily = family;
    s.textContent = 'mmmmmmmmmmlli WWWWWW';
    document.body.appendChild(s);
    const w = s.getBoundingClientRect().width;
    s.remove();
    return w;
  };

  // The baseline: a family that certainly does not exist, so the text falls
  // back. A family whose width differs from this resolved.
  const out = {};
  for (const f of %(families)s) {
    const base = measure('"Fury No Such Font 00", monospace');
    out[f] = {
      width: measure('"' + f + '", monospace'),
      base,
      resolved: measure('"' + f + '", monospace') !== base,
    };
  }
  out['#generic'] = {
    serif: measure('serif'),
    sans: measure('sans-serif'),
    mono: measure('monospace'),
    cursive: measure('cursive'),
  };
  return JSON.stringify(out);
})()
""" % {"families": json.dumps([ALLOWED, WITHHELD, NOWHERE])}


def main():
    claims = Claims("0050 — font fallback filter", CORE)

    # Establish first that this machine actually has both, or the test below
    # would pass on a Mac that simply lacks Papyrus.
    with launch(CORE, None) as s:
        bare = json.loads(s.js(PROBE))
        print(f"  unconfigured: {ALLOWED}={bare[ALLOWED]['resolved']} "
              f"{WITHHELD}={bare[WITHHELD]['resolved']} "
              f"{NOWHERE}={bare[NOWHERE]['resolved']}")

        if not bare[WITHHELD]["resolved"]:
            print(f"\nFAIL — this machine does not have {WITHHELD}, so the test "
                  f"below would pass for the wrong reason. Pick a family it has.")
            return 1

        claims.control(
            bare[ALLOWED]["resolved"] and bare[WITHHELD]["resolved"]
            and not bare[NOWHERE]["resolved"],
            f"an unconfigured build resolves both {ALLOWED} and {WITHHELD} and "
            f"not a family that exists nowhere — so this machine can tell the "
            f"three apart",
        )

    with launch(CORE, {"fonts": FONTS}) as s:
        got = json.loads(s.js(PROBE))
        print(f"  filtered:     {ALLOWED}={got[ALLOWED]['resolved']} "
              f"{WITHHELD}={got[WITHHELD]['resolved']} "
              f"{NOWHERE}={got[NOWHERE]['resolved']}")

        claims.check(got[ALLOWED]["resolved"],
                     f"{ALLOWED} is in the list and still resolves")
        claims.check(not got[WITHHELD]["resolved"],
                     f"{WITHHELD} is installed on this machine but not in the "
                     f"list, and no longer resolves — which is the filter doing "
                     f"its whole job")
        claims.check(not got[NOWHERE]["resolved"],
                     f"a family that exists nowhere still does not resolve — the "
                     f"filter did not make everything match")

        g, bg = got["#generic"], bare["#generic"]
        print(f"  generics filtered:     {g}")
        print(f"  generics unconfigured: {bg}")
        claims.check(all(v > 0 for v in g.values()),
                     f"every generic family still renders — a page whose body "
                     f"text disappears is not a quiet profile ({g})")
        claims.check(g["mono"] != g["serif"],
                     f"monospace and serif are still different faces "
                     f"({g['mono']} vs {g['serif']})")
        claims.check(len(set(g.values())) == len(set(bg.values())),
                     f"and the generics are as distinct from each other as on "
                     f"the unconfigured build — one face wearing four names is "
                     f"a machine that does not exist "
                     f"({len(set(g.values()))} distinct vs {len(set(bg.values()))})")
        claims.check(g == bg,
                     f"in fact they measure exactly what they do unfiltered, "
                     f"because a generic is not a family the page named")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
