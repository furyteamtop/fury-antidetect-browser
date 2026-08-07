#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""speechSynthesis.getVoices(), which is the OS in a list of strings.

Windows ships "Microsoft David", macOS ships an entirely different set, and
phase 0 measured a nineteen-voice gap between a clean Chromium and a real
Chrome on the same machine. The list moves with the persona or it contradicts
the platform the persona claims.

Narrowing only, by the same rule as fonts in 0050 and devices in 0060: a voice
can be hidden, never invented. An invented voice would fail at SPEAK time in a
way no real machine fails.

Three claims past the obvious one:

  * getVoices() is ASYNCHRONOUS in Chrome — the first call before
    `voiceschanged` returns an empty array. A script that reads it once and
    finds nothing has measured its own timing, not the browser, so this waits
    for the event.
  * a filter matching NOTHING is ignored rather than obeyed. An empty voice
    list is itself a fingerprint — no real desktop reports zero — and voice
    names are localised, so a persona captured in one locale will legitimately
    match none in another. The patch logs and keeps the real list; this checks
    that it does.
  * the kept voices are unmodified objects: name, lang, default and localService
    all still the platform's own.

Usage: core/verify/verify-0041.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

READ = """
(async () => {
  // getVoices() is empty until the list has loaded. Waiting for voiceschanged
  // is the difference between measuring the browser and measuring the clock.
  const voices = await new Promise((resolve) => {
    const now = speechSynthesis.getVoices();
    if (now.length) return resolve(now);
    speechSynthesis.onvoiceschanged = () => resolve(speechSynthesis.getVoices());
    setTimeout(() => resolve(speechSynthesis.getVoices()), 6000);
  });
  return JSON.stringify(voices.map(v => ({
    name: v.name, lang: v.lang, def: v.default, local: v.localService,
  })));
})()
"""


def main():
    claims = Claims("0041 — speech synthesis voices", CORE)

    with launch(CORE, None) as s:
        bare = json.loads(s.js(READ))
        names = [v["name"] for v in bare]
        print(f"  unconfigured: {len(bare)} voices, first few {names[:4]}")

        if len(names) < 3:
            print(f"\nFAIL — this host reports {len(names)} voices, too few to "
                  f"narrow. Nothing below would mean anything.")
            return 1

        claims.control(len(names) > 3,
                       f"an unconfigured build reports the machine's own "
                       f"{len(names)} voices, so a shorter list below came from "
                       f"the filter")

    keep = names[:2]
    with launch(CORE, {"speech": {"voices": keep}}) as s:
        got = json.loads(s.js(READ))
        got_names = [v["name"] for v in got]
        print(f"  filtered to {keep}: got {got_names}")

        claims.check(sorted(got_names) == sorted(keep),
                     f"the list is exactly the two voices asked for "
                     f"(got {got_names})")
        claims.check(len(got) < len(bare),
                     f"which is fewer than the machine has "
                     f"({len(got)} of {len(bare)})")

        kept = {v["name"]: v for v in got}
        original = {v["name"]: v for v in bare if v["name"] in kept}
        claims.check(all(kept[n] == original[n] for n in kept),
                     "and the voices that survived are unchanged — name, lang, "
                     "default and localService all still the platform's own")

    # A filter that matches nothing.
    with launch(CORE, {"speech": {"voices": ["Fury No Such Voice 4471"]}}) as s:
        none = json.loads(s.js(READ))
        print(f"  filtered to a name nothing matches: {len(none)} voices")
        claims.check(len(none) == len(bare),
                     f"a filter matching no installed voice is ignored, not "
                     f"obeyed — zero voices is a fingerprint no real desktop "
                     f"has, and voice names are localised, so a persona "
                     f"captured elsewhere will legitimately match none "
                     f"({len(none)} of {len(bare)})")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
