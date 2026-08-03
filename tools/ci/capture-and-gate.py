#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Capture every execution context from the built browser, then gate it.

What self-hosted.yml runs, and what anyone can run by hand after a build.

No proxy: the ServiceWorker script must be same-origin, so the page has to be
the collector's, and a Fury profile sends loopback through the relay. The
network is not what is under test here — this is the fingerprint question, so
the core runs with a config on fd 3 and no relay at all.

    python3 tools/ci/capture-and-gate.py [core-binary]
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "core/verify"))
from cdp import browser  # noqa: E402


def find_core():
    if len(sys.argv) > 1:
        return sys.argv[1]
    for out in ("macos-arm64", "macos-arm64-lowmem"):
        for name in ("Fury", "Chromium"):
            p = ROOT / f"core/src/out/{out}/{name}.app/Contents/MacOS/{name}"
            if p.is_file():
                return str(p)
    print("!! no built core found; pass the path", file=sys.stderr)
    sys.exit(2)


def derive_config(dest):
    """A real persona config, from the catalogue, via the agent's own CLI."""
    out = subprocess.run(
        ["cargo", "run", "-q", "-p", "fury-agent", "--", "check-fingerprint", "-"],
        cwd=ROOT, capture_output=True, text=True,
    )
    # check-fingerprint validates the built-in sample and prints it; if the
    # shape ever changes this fails loudly rather than capturing a browser that
    # was never configured.
    if out.returncode != 0:
        print("!! could not derive a config:", out.stderr[-400:], file=sys.stderr)
        sys.exit(2)
    dest.write_text(out.stdout)
    return dest


CORE = find_core()
cfg = pathlib.Path(tempfile.mkdtemp()) / "config.json"
derive_config(cfg)

d = tempfile.mkdtemp(prefix="fury-ci-")
fd = os.open(cfg, os.O_RDONLY)
os.set_inheritable(fd, True)
p = subprocess.Popen(
    [CORE, f"--user-data-dir={d}", "--fury-fp-fd=3", "--remote-debugging-port=0",
     "--no-first-run", "--no-default-browser-check", "--disable-field-trial-config",
     "--lang=en-US", "--window-position=-4000,-4000", "--window-size=900,700"],
    preexec_fn=(lambda: os.dup2(fd, 3)), close_fds=False,
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
)
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
        print(f"!! the core exited {p.returncode}", file=sys.stderr)
        sys.exit(1)
    time.sleep(0.2)
time.sleep(1)

ws = browser(port)
t = ws.call("Target.createTarget", {"url": "http://127.0.0.1:8731/probe.html"})["targetId"]
s = ws.call("Target.attachToTarget", {"targetId": t, "flatten": True})["sessionId"]
ws.call("Runtime.enable", session=s)
time.sleep(4)

probe = (ROOT / "tools/detect-suite/probe.js").read_text()
ws.call("Runtime.evaluate", {"expression": probe, "returnByValue": True}, session=s)
r = ws.call("Runtime.evaluate", {"expression": "furyProbe()", "awaitPromise": True,
                                 "returnByValue": True, "timeout": 120000}, session=s)
dump = r["result"].get("value")
p.terminate()

if not isinstance(dump, dict):
    print("!! the probe returned nothing", file=sys.stderr)
    sys.exit(1)

cc = dump.get("crossContext", {})
origin = (dump.get("contexts", {}).get("main") or {}).get("origin")
print(f"  contexts: {len(cc.get('contextsProbed') or [])}")
print(f"  disagreements: {len(cc.get('disagreements') or [])}")

# An "origin: null" here means the page did not load — check the collector
# before anything else. That mistake cost an hour once and looked exactly like
# a patch having broken the renderer.
if cc.get("contextsProbed") and len(cc["contextsProbed"]) < 8:
    print("!! fewer than eight contexts — is the collector serving probe.html?",
          file=sys.stderr)

out = ROOT / "tools/detect-suite/baselines/ci-capture.json"
out.write_text(json.dumps(dump, indent=2, sort_keys=True))

if cc.get("disagreements"):
    print(json.dumps(cc["disagreements"], indent=1)[:1200], file=sys.stderr)
    sys.exit(1)

gate = subprocess.run(["cargo", "run", "-q", "-p", "fury-detect", "--", "gate", str(out)],
                      cwd=ROOT)
sys.exit(gate.returncode)
