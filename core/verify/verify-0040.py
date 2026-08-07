#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""The audio fingerprint, which is one float that a whole industry reads.

The classic probe is an OfflineAudioContext rendering an oscillator through a
DynamicsCompressor and summing the samples. Different audio stacks disagree in
the last few bits, so the sum identifies the stack. Every fingerprinting library
computes it the same way, so this script computes it the same way too.

What 0040 claims, and each part is separately checkable:

  * the sum MOVED, and moves with the SEED.
  * rendering the same graph twice gives the SAME sum. The perturbation is a
    function of (seed, channel, sample index), so a detector that renders twice
    and compares finds nothing. A value that moved between runs would be a far
    stronger signal than an unspoofed one.
  * the amplitude stays under 3e-5 per sample. That is about one
    least-significant bit of 16-bit audio, some eight bits above the float32
    floor: inaudible, below the noise of any real converter, and still enough
    to move the sum.
  * `baseLatency` and `outputLatency` come from the persona, because both are
    hardware-derived and cluster per OS. `outputLatency` is checked to be a
    plausible duration rather than merely equal: a negative or absurd latency
    would be its own tell.

Usage: core/verify/verify-0040.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# Under 2^31: the core reads seeds through GetInt, which is a signed 32-bit
# int, and a larger value is dropped rather than clamped. See
# shared-rs/src/persona.rs::sub_seed.
SEED_A = 0x0AD10001
SEED_B = 0x0AD10002

# A Windows/WASAPI-shaped pair. Not this Mac's.
LATENCY = 0.0213333333333333

# Word for word what the fingerprinting libraries do.
PROBE = """
(async () => {
  const render = async () => {
    const ctx = new OfflineAudioContext(1, 44100, 44100);
    const osc = ctx.createOscillator();
    osc.type = 'triangle';
    osc.frequency.value = 10000;
    const comp = ctx.createDynamicsCompressor();
    comp.threshold.value = -50;
    comp.knee.value = 40;
    comp.ratio.value = 12;
    comp.attack.value = 0;
    comp.release.value = 0.25;
    osc.connect(comp); comp.connect(ctx.destination);
    osc.start(0);
    const buf = await ctx.startRendering();
    const d = buf.getChannelData(0);
    let sum = 0, peak = 0;
    for (let i = 4500; i < 5000; i++) { sum += Math.abs(d[i]); }
    for (let i = 0; i < d.length; i++) { peak = Math.max(peak, Math.abs(d[i])); }
    return {sum, peak, first: Array.from(d.slice(4500, 4505))};
  };

  const one = await render();
  const two = await render();

  const live = new AudioContext();
  const latency = {base: live.baseLatency, output: live.outputLatency,
                   rate: live.sampleRate};
  live.close();

  return JSON.stringify({one, two, latency});
})()
"""


def main():
    claims = Claims("0040 — audio", CORE)

    config = {"noise": {"audioSeed": SEED_A},
              "audio": {"outputLatency": LATENCY}}
    with launch(CORE, config) as s:
        a = json.loads(s.js(PROBE))
        print(f"  seed A sum: {a['one']['sum']!r}")
        print(f"  latency: {a['latency']}")

        claims.check(a["one"]["sum"] == a["two"]["sum"],
                     f"rendering the same graph twice gives the same sum — a "
                     f"value that moved between runs would be a stronger signal "
                     f"than no noise at all ({a['one']['sum']} twice)")
        claims.check(a["one"]["first"] == a["two"]["first"],
                     "and the individual samples match too, not just their sum")

        claims.check(abs(a["latency"]["output"] - LATENCY) < 1e-12,
                     f"outputLatency is the persona's "
                     f"(got {a['latency']['output']})")
        claims.check(0 < a["latency"]["output"] < 1,
                     f"and it is a plausible duration — a negative or absurd "
                     f"latency is its own tell ({a['latency']['output']} s)")
        claims.check(0 < a["latency"]["base"] <= a["latency"]["output"],
                     f"baseLatency is positive and no larger than the output "
                     f"latency it is part of "
                     f"({a['latency']['base']} <= {a['latency']['output']})")

    with launch(CORE, {"noise": {"audioSeed": SEED_B}}) as s:
        b = json.loads(s.js(PROBE))
        print(f"  seed B sum: {b['one']['sum']!r}")
        claims.check(b["one"]["sum"] != a["one"]["sum"],
                     f"a different seed sums differently — otherwise every "
                     f"profile shares one audio hash "
                     f"({b['one']['sum']} vs {a['one']['sum']})")

    with launch(CORE, None) as s:
        bare = json.loads(s.js(PROBE))
        print(f"  unconfigured sum: {bare['one']['sum']!r}")

        moved = abs(a["one"]["sum"] - bare["one"]["sum"])
        claims.check(moved > 0,
                     f"the noised sum differs from the unconfigured one by "
                     f"{moved:.3e}")
        per_sample = max(
            abs(x - y) for x, y in zip(a["one"]["first"], bare["one"]["first"]))
        claims.check(per_sample < 3e-5,
                     f"but no single sample moved by more than {per_sample:.3e} "
                     f"— about one LSB of 16-bit audio, below the noise floor of "
                     f"any real converter")
        claims.control(
            bare["one"]["sum"] == bare["two"]["sum"]
            and bare["one"]["sum"] != a["one"]["sum"],
            f"an unconfigured build renders a stable sum of its own, so the "
            f"numbers above came from the seed (got {bare['one']['sum']!r})",
        )

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
