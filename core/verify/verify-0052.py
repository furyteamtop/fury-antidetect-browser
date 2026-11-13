#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""queryLocalFonts() answers to the same font rule as rendering.

0050 and 0051 make a hidden family measure absent: canvas, offsetWidth and
local() all fall back as on a machine without it. Local Font Access is the
one API that LISTS fonts, and before 0052 it read the machine's list past
that rule, so a page holding the local-fonts permission saw a family in the
list that it could not render. Issue 33 measured it on 0.5.1: 15 hidden
families at 0/15 by width and 15/15 in queryLocalFonts().

Claims:

  * control: an unconfigured build lists the family, so the test below is not
    passing on a machine that simply lacks it.
  * with the family in fontsHidden (in other case, the rule is
    case-insensitive), no face of it is listed, and the width probe agrees.
  * nothing else went: the list is the unconfigured one minus exactly that
    family's faces.
  * the legacy allowlist ("fonts" alone) lists only its own families.
  * an empty fontsHidden lists what an unconfigured build lists.

Usage: core/verify/verify-0052.py <core binary> [allowed family] [hidden family]
On Windows, for example: <core binary> Georgia Arial.
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import PAGE, Claims, launch  # noqa: E402

CORE = sys.argv[1]
ALLOWED = sys.argv[2] if len(sys.argv) > 2 else "Georgia"
WITHHELD = sys.argv[3] if len(sys.argv) > 3 else "Papyrus"

QUERY = """(async () => {
  try {
    const fonts = await window.queryLocalFonts();
    return JSON.stringify(fonts.map(f => [f.family, f.postscriptName]));
  } catch (e) {
    return JSON.stringify({error: String(e)});
  }
})()"""

WIDTH = """(() => {
  const measure = (family) => {
    const s = document.createElement('span');
    s.style.cssText = 'position:absolute;left:-9999px;font-size:72px;white-space:nowrap';
    s.style.fontFamily = family;
    s.textContent = 'mmmmmmmmmmlli WWWWWW';
    document.body.appendChild(s);
    const w = s.offsetWidth;
    s.remove();
    return w;
  };
  return measure(%s + ', monospace') !== measure('monospace');
})()""" % json.dumps(json.dumps(WITHHELD))


def listed(s):
    s.ws.call("Browser.grantPermissions",
              {"origin": PAGE, "permissions": ["localFonts"]})
    got = json.loads(s.js(QUERY))
    if isinstance(got, dict):
        raise RuntimeError(f"queryLocalFonts() failed: {got['error']}")
    return [tuple(x) for x in got]


def faces_of(fonts, family):
    return {f for f in fonts if f[0].lower() == family.lower()}


def main():
    claims = Claims("0052 — Local Font Access filter", CORE)

    with launch(CORE, None) as s:
        bare = listed(s)
    families = {f[0].lower() for f in bare}
    print(f"  unconfigured: {len(bare)} faces, {len(families)} families")
    if WITHHELD.lower() not in families or ALLOWED.lower() not in families:
        print(f"\nFAIL — this machine does not list both {ALLOWED} and "
              f"{WITHHELD}, so the test below would pass for the wrong reason.")
        return 1
    withheld_faces = faces_of(bare, WITHHELD)
    claims.control(
        len(withheld_faces) > 0,
        f"an unconfigured build lists {WITHHELD} "
        f"({len(withheld_faces)} faces) — so hiding it has something to remove",
    )

    with launch(CORE, {"fonts": [ALLOWED, WITHHELD],
                       "fontsHidden": [WITHHELD.swapcase()]}) as s:
        got = listed(s)
        renders = s.js(WIDTH)
    print(f"  hidden {WITHHELD.swapcase()!r}: {len(got)} faces")
    claims.check(not faces_of(got, WITHHELD),
                 f"no face of {WITHHELD} is listed once fontsHidden names it")
    claims.check(renders is False,
                 f"and the width probe agrees it is absent, so the list and "
                 f"rendering give one answer")
    claims.check(sorted(got) == sorted(set(bare) - withheld_faces),
                 f"everything else is listed exactly as unconfigured "
                 f"({len(got)} = {len(bare)} - {len(withheld_faces)})")

    allow = [ALLOWED, "Arial", "Times New Roman"]
    with launch(CORE, {"fonts": allow}) as s:
        got = listed(s)
    extra = sorted({f[0] for f in got if f[0].lower() not in
                    {a.lower() for a in allow}})
    claims.check(got and not extra,
                 f"the legacy allowlist lists only its own families "
                 f"({len(got)} faces, outside it: {extra[:5]})")

    with launch(CORE, {"fonts": [ALLOWED], "fontsHidden": []}) as s:
        got = listed(s)
    claims.check(sorted(got) == sorted(bare),
                 "an empty blocklist lists what an unconfigured build lists")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
