#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Does the config reach every context, and stay out of everywhere else?

Patch 0001 is the floor the other twenty-six stand on. If the config does not
arrive, every one of them falls back to stock Chromium and the profile looks
configured while being nothing of the kind.

Three claims, and the second is the one that matters:

  1. The browser reads it at all.

  2. Every CONTEXT gets the same answer. A Worker, a ServiceWorker and an
     iframe each run in their own process or their own global, and a config
     that reaches the page and not the worker is not a weaker spoof — it is a
     CONTRADICTION, and a page that reads `navigator.hardwareConcurrency` in
     both and compares has found something no amount of main-frame spoofing
     hides. This is what the shared-memory republication in 0001 exists for.

  3. It is not in argv. The whole persona would otherwise be readable by any
     process on the machine, which for a product whose claim is that the
     browser is not distinguishable would be an odd way to start.

Usage: core/verify/verify-0001.py <core binary>
"""

import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# Deliberately not this machine's value. Every Mac this is developed on reports
# 8, 10 or 12; 6 is a real number no host here produces, so an answer of 6 came
# from the config rather than from the hardware.
CORES = 6

# The same question asked in four places at once.
ACROSS_CONTEXTS = """
(async () => {
  const ask = 'navigator.hardwareConcurrency';
  const out = { main: eval(ask) };

  // A dedicated Worker: its own thread, its own global.
  const workerSrc = `postMessage(${ask})`;
  const w = new Worker(URL.createObjectURL(new Blob([workerSrc])));
  out.worker = await new Promise((r) => { w.onmessage = (e) => r(e.data); });
  w.terminate();

  // An iframe: a separate document, and about:blank inherits differently from
  // srcdoc, so both are asked.
  const blank = document.createElement('iframe');
  blank.src = 'about:blank';
  document.body.appendChild(blank);
  out.iframeBlank = blank.contentWindow.navigator.hardwareConcurrency;
  blank.remove();

  const srcdoc = document.createElement('iframe');
  srcdoc.srcdoc = '<html><body></body></html>';
  document.body.appendChild(srcdoc);
  await new Promise((r) => { srcdoc.onload = r; setTimeout(r, 2000); });
  out.iframeSrcdoc = srcdoc.contentWindow.navigator.hardwareConcurrency;
  srcdoc.remove();

  return JSON.stringify(out);
})()
"""


def main():
    claims = Claims("0001 — the config reaches the browser", CORE)

    with launch(CORE, {"navigator": {"hardwareConcurrency": CORES}}) as s:
        got = __import__("json").loads(s.js(ACROSS_CONTEXTS))
        print(f"  measured: {got}")

        claims.check(got["main"] == CORES,
                     f"the page reads {CORES} (got {got['main']})")

        # The claim 0001 exists for. Not "the worker is also spoofed" but "the
        # worker AGREES" — a disagreement is the tell.
        contexts = ["main", "worker", "iframeBlank", "iframeSrcdoc"]
        values = {c: got[c] for c in contexts}
        claims.check(
            len(set(values.values())) == 1,
            f"every context agrees — a disagreement is the tell: {values}",
        )
        claims.check(all(v == CORES for v in values.values()),
                     f"and all of them read {CORES}")

        # And the persona is not on the command line, where `ps` would have it.
        argv = subprocess.run(
            ["ps", "-o", "command=", "-p", str(s.proc.pid)],
            capture_output=True, text=True,
        ).stdout
        claims.check(
            "--fury-fp-fd=3" in argv,
            "argv names the descriptor the config arrived on",
        )
        claims.check(
            "hardwareConcurrency" not in argv and f"={CORES}" not in argv,
            "and does not contain the persona itself",
        )

    # The control. Without it, every check above also passes on a build where
    # 0001 does nothing and the host happens to have six cores.
    with launch(CORE, None) as s:
        bare = s.js("navigator.hardwareConcurrency")
        print(f"  unconfigured: {bare}")
        claims.control(
            bare != CORES,
            f"an unconfigured build reports the host's own count, not {CORES} "
            f"(got {bare})",
        )

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
