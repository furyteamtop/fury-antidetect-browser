#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""The WebGPU adapter's feature set — narrowed, never widened.

`navigator.gpu.requestAdapter()` then `adapter.features` is a set of strings
that differs by GPU and driver, and it is read by exactly the scripts that read
the WebGL renderer string. A persona whose WebGL says NVIDIA while its WebGPU
feature set is this Mac's has described a machine that does not exist.

The direction is the whole design, and it is the same rule as fonts (0050),
voices (0041) and devices (0060): a feature may be HIDDEN, never invented.
Hiding one is honest in its consequences — a pipeline using it fails to create,
exactly as on hardware without it. Advertising one that is not there would fail
at draw time, in a way no real machine fails.

So the two claims that matter are a narrowing that works and a widening that is
refused.

Usage: core/verify/verify-0032.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

INVENTED = "fury-nonexistent-feature"

READ = """
(async () => {
  if (!navigator.gpu) return JSON.stringify({error: 'no webgpu'});
  const a = await navigator.gpu.requestAdapter();
  if (!a) return JSON.stringify({error: 'no adapter'});
  const info = a.info || {};
  return JSON.stringify({
    features: Array.from(a.features).sort(),
    limits: {maxTextureDimension2D: a.limits.maxTextureDimension2D},
    vendor: info.vendor, architecture: info.architecture,
  });
})()
"""


def main():
    claims = Claims("0032 — WebGPU adapter features", CORE)

    with launch(CORE, None) as s:
        bare = json.loads(s.js(READ))
        if bare.get("error"):
            print(f"\nFAIL — {bare['error']}: this host exposes no WebGPU "
                  f"adapter, so nothing below would mean anything.")
            return 1
        print(f"  unconfigured: {len(bare['features'])} features")
        print(f"    {bare['features']}")

        if len(bare["features"]) < 2:
            print(f"\nFAIL — {len(bare['features'])} features is too few to "
                  f"narrow.")
            return 1

        claims.control(len(bare["features"]) >= 2,
                       f"an unconfigured build reports the machine's own "
                       f"{len(bare['features'])} features, so a shorter list "
                       f"below came from the config")

    keep = bare["features"][:2]
    with launch(CORE, {"gpu": {"webgpu": {"features": keep}}}) as s:
        got = json.loads(s.js(READ))
        print(f"  narrowed to {keep}: {got['features']}")
        claims.check(got["features"] == sorted(keep),
                     f"the adapter reports exactly the two asked for "
                     f"(got {got['features']})")
        claims.check(len(got["features"]) < len(bare["features"]),
                     f"which is fewer than the hardware has "
                     f"({len(got['features'])} of {len(bare['features'])})")
        claims.check(got["limits"]["maxTextureDimension2D"]
                     == bare["limits"]["maxTextureDimension2D"],
                     "and the limits are untouched — this patch narrows the "
                     "feature set, and says so")

    # The direction that must not work.
    widened = bare["features"] + [INVENTED]
    with launch(CORE, {"gpu": {"webgpu": {"features": widened}}}) as s:
        more = json.loads(s.js(READ))
        print(f"  asked for an invented feature: {len(more['features'])} features")
        claims.check(INVENTED not in more["features"],
                     f"a feature the hardware does not have is NOT invented — "
                     f"it would fail at draw time in a way no real machine "
                     f"fails (got {more['features']})")
        claims.check(more["features"] == bare["features"],
                     f"and asking for everything the machine has plus one "
                     f"leaves the real list untouched "
                     f"({len(more['features'])} of {len(bare['features'])})")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
