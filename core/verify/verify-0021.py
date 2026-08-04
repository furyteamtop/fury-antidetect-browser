#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Does patch 0021 make the scrollbar the width the persona claims?

The tell is one subtraction. A page puts more content in an element than fits,
reads `offsetWidth - clientWidth`, and gets ~15-17 px on Windows against 0 on
macOS, where scrollbars are overlays. No amount of navigator spoofing touches
it, because it is a layout measurement rather than a property.

Two things this checks that the first version of the patch got wrong, and both
were found by reading rather than by running — which is why they are assertions
now:

  - `scrollbar-width: thin` reports TWO THIRDS of the width in every real
    browser. The patch returned the configured number for both values, so a
    page that set one CSS property got the same answer twice and had us. macOS
    has the same distinction in its own table: 15 for auto, 11 for thin.

  - the width and the overlay flag have to move together. An overlay scrollbar
    reserves no layout space, so a configured width that layout never reserves
    is not a width and the measurement stays 0.

The last check is the control this repository insists on: an unconfigured build
must report something DIFFERENT. A check that passes when nothing is wired up
is not a check.

Usage: core/verify/verify-0021.py <core binary>
See core/verify/README.md.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cdp import browser  # noqa: E402

CORE = sys.argv[1]
results = []

# What the persona claims. 17 is what Windows 10/11 report at 100% scaling, and
# it is deliberately not a number this machine would produce on its own.
WIDTH = 17

# The measurement a detector makes, for both values of the CSS property, in the
# main frame and in an iframe. The iframe matters: a config that reaches the top
# document and not the one below it is the cross-context disagreement the whole
# probe suite exists to catch.
MEASURE = """
(() => {
  const measure = (doc, cssWidth) => {
    const el = doc.createElement('div');
    el.style.cssText = 'width:100px;height:100px;overflow:scroll;position:absolute;top:-9999px';
    if (cssWidth) el.style.scrollbarWidth = cssWidth;
    const inner = doc.createElement('div');
    inner.style.cssText = 'width:100%;height:400px';
    el.appendChild(inner);
    doc.body.appendChild(el);
    const w = el.offsetWidth - el.clientWidth;
    el.remove();
    return w;
  };
  const f = document.createElement('iframe');
  f.src = 'about:blank';
  document.body.appendChild(f);
  const d = f.contentDocument;
  const out = {
    auto: measure(document, null),
    thin: measure(document, 'thin'),
    none: measure(document, 'none'),
    iframeAuto: measure(d, null),
  };
  f.remove();
  return JSON.stringify(out);
})()
"""


def check(ok, text):
    results.append((ok, text))
    print(f"  {'OK  ' if ok else 'FAIL'} {text}", flush=True)


def launch(config):
    """Start the core on a real page and attach. Same shape as verify-0121."""
    d = tempfile.mkdtemp(prefix="fury-0021-")
    args = [CORE, f"--user-data-dir={d}", "--remote-debugging-port=0",
            "--no-first-run", "--no-default-browser-check",
            "--window-position=-4000,-4000", "--window-size=600,400"]
    fd = None
    if config is not None:
        cfg = os.path.join(d, "config.json")
        with open(cfg, "w") as f:
            json.dump(config, f)
        fd = os.open(cfg, os.O_RDONLY)
        os.set_inheritable(fd, True)
        args.append("--fury-fp-fd=3")
    p = subprocess.Popen(args, preexec_fn=((lambda: os.dup2(fd, 3)) if fd else None),
                         close_fds=False, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL)
    if fd is not None:
        os.close(fd)

    pf = os.path.join(d, "DevToolsActivePort")
    port = None
    for _ in range(300):
        if os.path.exists(pf):
            try:
                port = int(open(pf).readline().strip())
            except ValueError:
                port = None
            if port:
                break
        if p.poll() is not None:
            raise RuntimeError(f"core exited {p.returncode}")
        time.sleep(0.2)
    time.sleep(1)
    ws = browser(port)
    # A real page rather than about:blank: layout on about:blank is a special
    # case in more than one engine, and a scrollbar measurement taken there is
    # not the measurement a site takes.
    t = ws.call("Target.createTarget", {"url": "https://example.com"})["targetId"]
    s = ws.call("Target.attachToTarget", {"targetId": t, "flatten": True})["sessionId"]
    ws.call("Runtime.enable", session=s)
    time.sleep(2.5)
    return p, ws, s, d


def read(config):
    p, ws, s, d = launch(config)
    try:
        r = ws.call("Runtime.evaluate",
                    {"expression": MEASURE, "awaitPromise": True,
                     "returnByValue": True, "timeout": 20000}, session=s)
        if "exceptionDetails" in r:
            raise RuntimeError(str(r["exceptionDetails"].get("text")))
        # `call` already unwraps the outer "result"; what comes back is the
        # RemoteObject.
        return json.loads(r["result"]["value"])
    finally:
        ws.close()
        p.terminate()
        p.wait(timeout=10)
        shutil.rmtree(d, ignore_errors=True)


def main():
    print(f"verify 0021 — scrollbar width, against {CORE}")

    print("\nconfigured to claim a Windows scrollbar")
    got = read({"schema_version": 1, "screen": {"scrollbarWidth": WIDTH}})
    print(f"  measured: {got}")

    check(got["auto"] == WIDTH,
          f"auto reserves {WIDTH} px in layout (got {got['auto']})")

    # int(17 * 2/3) = 11 on the macOS path, ClampRound(17 * 2/3) = 11 on the
    # Windows one. Both land on 11; the assertion allows either rounding rather
    # than pinning a platform's arithmetic.
    want_thin = {int(WIDTH * 2 / 3), round(WIDTH * 2 / 3)}
    check(got["thin"] in want_thin,
          f"scrollbar-width: thin reports two thirds, {sorted(want_thin)} "
          f"(got {got['thin']})")

    check(got["thin"] != got["auto"],
          "thin and auto differ — the same number twice is the tell this patch "
          f"was missing (thin {got['thin']}, auto {got['auto']})")

    check(got["none"] == 0,
          f"scrollbar-width: none still reserves nothing (got {got['none']})")

    check(got["iframeAuto"] == WIDTH,
          f"an iframe measures the same {WIDTH} px (got {got['iframeAuto']})")

    print("\nconfigured to claim overlay scrollbars")
    overlay = read({"schema_version": 1, "screen": {"scrollbarWidth": 0}})
    print(f"  measured: {overlay}")
    check(overlay["auto"] == 0,
          f"a width of 0 turns overlay back on and reserves nothing "
          f"(got {overlay['auto']})")

    # The control. Without it every assertion above would also pass on a build
    # where the patch does nothing and the host happens to agree.
    print("\nunconfigured — this must NOT match")
    # A config with no scrollbarWidth in it, rather than no config at all: the
    # comparison has to be "the patch is not told what to say" and not "the
    # plumbing is absent", or the control passes for the wrong reason.
    bare = read({"schema_version": 1})
    print(f"  measured: {bare}")
    check(bare["auto"] != WIDTH,
          f"an unconfigured build reports the host's own width, not {WIDTH} "
          f"(got {bare['auto']}) — if this fails the checks above prove nothing")

    failed = [t for ok, t in results if not ok]
    print()
    if failed:
        print(f"FAIL — {len(failed)} of {len(results)} checks failed")
        return 1
    print(f"PASS — {len(results)} checks")
    return 0


if __name__ == "__main__":
    sys.exit(main())
