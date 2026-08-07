#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Element geometry: perturbed, but never drifting.

`getBoundingClientRect()` returns sub-pixel numbers that depend on the font
stack, the rasteriser and the layout engine, which is why measuring a string of
text is a fingerprint. 0033 perturbs them — and the danger is entirely in HOW.

Four claims, and the second is the one worth having:

  * the values MOVED. A rect identical to the unconfigured build is no spoof.
  * they do NOT DRIFT. The same element measured a hundred times gives one
    answer. The perturbation is derived from the value itself, so measuring
    again re-derives the same offset — and a detector that loops over
    getBoundingClientRect() watching for movement finds none. This is the claim
    that separates 0033 from the naive version of itself, which would make the
    browser far more conspicuous than an unspoofed one.
  * `getClientRects()` and `getBoundingClientRect()` AGREE, because they read
    through the same override; a single-line box's bounding rect is its one
    client rect.
  * the geometry is still INTERNALLY CONSISTENT: right - left equals width,
    bottom - top equals height. An override applied to each field
    independently would break the subtraction, which is three lines of JS to
    check.

And the perturbation must be SMALL — under a hundredth of a pixel, so nothing
that lays out a page notices, while every hash of the numbers changes.

Usage: core/verify/verify-0033.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# Under 2^31, and that is load-bearing rather than arbitrary: the core reads
# seeds through FuryConfig::GetInt, which is a SIGNED 32-bit int. A larger value
# is not clamped — it is dropped, the seed lookup answers "not configured", and
# the surface goes entirely unnoised while nothing reports a problem. See the
# mask in shared-rs/src/persona.rs::sub_seed, which is what keeps real profiles
# inside the range.
SEED_A = 0x41EC7001
SEED_B = 0x41EC7002

READ = """
(() => {
  // Text, because text is what a geometry fingerprint actually measures: the
  // width of a rendered string is the font stack in one number.
  const d = document.createElement('div');
  d.style.cssText = 'position:absolute;left:37px;top:53px;font:16px serif;white-space:nowrap';
  d.textContent = 'Fury \\u2014 geometry 0033 \\u2014 mmmmiiiillll';
  document.body.appendChild(d);

  const r = d.getBoundingClientRect();
  const rect = {
    x: r.x, y: r.y, width: r.width, height: r.height,
    top: r.top, right: r.right, bottom: r.bottom, left: r.left,
  };

  // A hundred more reads. If any of them differs, the noise depends on
  // something other than the value.
  let drifted = 0;
  for (let i = 0; i < 100; i++) {
    const again = d.getBoundingClientRect();
    if (again.width !== r.width || again.left !== r.left ||
        again.height !== r.height || again.top !== r.top) {
      drifted++;
    }
  }

  const list = d.getClientRects();
  const first = list.length ? {
    width: list[0].width, height: list[0].height,
    left: list[0].left, top: list[0].top,
  } : null;

  d.remove();
  return JSON.stringify({rect, drifted, rectCount: list.length, first});
})()
"""


def main():
    claims = Claims("0033 — client rects", CORE)

    with launch(CORE, {"noise": {"clientRectsSeed": SEED_A}}) as s:
        a = json.loads(s.js(READ))
        print(f"  seed A: width {a['rect']['width']!r} left {a['rect']['left']!r}")

        claims.check(a["drifted"] == 0,
                     f"a hundred further reads of the same element return the "
                     f"same numbers — noise that drifts is a stronger signal "
                     f"than no noise ({a['drifted']} differed)")

        r = a["rect"]
        claims.check(abs((r["right"] - r["left"]) - r["width"]) < 1e-9,
                     f"right - left is still width — the fields were not "
                     f"perturbed independently "
                     f"({r['right'] - r['left']} vs {r['width']})")
        claims.check(abs((r["bottom"] - r["top"]) - r["height"]) < 1e-9,
                     f"and bottom - top is still height "
                     f"({r['bottom'] - r['top']} vs {r['height']})")
        claims.check(r["x"] == r["left"] and r["y"] == r["top"],
                     "x and y still equal left and top, as they do in every "
                     "browser for a rect with no transform")

        claims.check(a["rectCount"] == 1 and a["first"] is not None,
                     f"a single-line box has exactly one client rect "
                     f"(got {a['rectCount']})")
        claims.check(
            abs(a["first"]["width"] - r["width"]) < 1e-9
            and abs(a["first"]["left"] - r["left"]) < 1e-9,
            "getClientRects()[0] and getBoundingClientRect() agree — they read "
            "through the same override",
        )

    with launch(CORE, {"noise": {"clientRectsSeed": SEED_B}}) as s:
        b = json.loads(s.js(READ))
        print(f"  seed B: width {b['rect']['width']!r} left {b['rect']['left']!r}")
        claims.check(b["rect"]["width"] != a["rect"]["width"],
                     f"a different seed measures a different width — otherwise "
                     f"every profile shares one geometry hash "
                     f"({b['rect']['width']} vs {a['rect']['width']})")

    with launch(CORE, None) as s:
        bare = json.loads(s.js(READ))
        print(f"  unconfigured: width {bare['rect']['width']!r} "
              f"left {bare['rect']['left']!r}")

        moved = abs(a["rect"]["width"] - bare["rect"]["width"])
        claims.check(moved > 0,
                     f"the configured build measures a different width from the "
                     f"unconfigured one — it moved by {moved}")
        claims.check(moved < 0.01,
                     f"but by less than a hundredth of a pixel, so nothing that "
                     f"lays out a page notices ({moved})")
        claims.control(
            bare["drifted"] == 0 and bare["rect"]["width"] != a["rect"]["width"],
            f"an unconfigured build is stable too and measures its own width — "
            f"stability is the floor, not the achievement "
            f"(got {bare['rect']['width']})",
        )

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
