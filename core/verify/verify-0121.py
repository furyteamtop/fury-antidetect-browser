#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Does patch 0121 stop every profile on this machine reporting one battery?

The leak was invisible to a parity check, which is why it survived so long:
measured 02.08.2026, our build and real Chrome 150 both answered
navigator.getBattery() with charging=false, level=0.60, chargingTime=Infinity,
dischargingTime=34680 — identical to the second, because both were reading this
MacBook. Perfect parity, and a machine-wide correlator handed to every profile.

The check that matters most here is the last one. Overriding the four getters
instead of the status the dispatcher publishes would pass every value check on
this page and still fire levelchange off the host's real cell.

Usage: core/verify/verify-0121.py <core binary>
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


def check(ok, text):
    results.append((ok, text))
    print(f"  {'OK  ' if ok else 'FAIL'} {text}", flush=True)


READ = """
(async () => {
  if (!('getBattery' in navigator)) return {present: false};
  const b = await navigator.getBattery();
  return {present: true, charging: b.charging, level: b.level,
          chargingTime: b.chargingTime === Infinity ? 'Infinity' : b.chargingTime,
          dischargingTime: b.dischargingTime === Infinity ? 'Infinity' : b.dischargingTime,
          native: /\\[native code\\]/.test(String(navigator.getBattery))};
})()
"""

# The same four values from a second execution context. BatteryManager is
# Window-only (battery_manager.idl Exposed=Window), so there is no worker to
# reconcile — but an iframe has its own document, its own Navigator and its own
# BatteryDispatcher, and a config that reaches the top frame and not the frame
# below it is exactly the cross-context disagreement probe.js was written to
# catch. Driven directly through contentWindow rather than by postMessage: a
# cross-origin child cannot answer without a listener we have no way to inject,
# and a check that quietly skips is a check that is not run.
IN_IFRAME = """
new Promise(r => {
  const f = document.createElement('iframe');
  f.onload = async () => {
    try {
      const b = await f.contentWindow.navigator.getBattery();
      r({charging: b.charging, level: b.level,
         chargingTime: b.chargingTime === Infinity ? 'Infinity' : b.chargingTime,
         dischargingTime: b.dischargingTime === Infinity ? 'Infinity' : b.dischargingTime});
    } catch (e) { r({threw: String(e)}); }
  };
  f.src = 'about:blank';
  document.body.appendChild(f);
  setTimeout(() => r({timedOut: true}), 8000);
})
"""

# Three minutes is not available to a test, so this watches for the events
# themselves rather than for a level change: with the real cell behind it the
# dispatcher republishes on every mojo round trip, and any republished value
# that differs from the last one fires here.
WATCH = """
new Promise(async r => {
  const b = await navigator.getBattery();
  const fired = [];
  for (const e of ['chargingchange', 'levelchange',
                   'chargingtimechange', 'dischargingtimechange'])
    b.addEventListener(e, () => fired.push(e));
  const first = [b.charging, b.level, b.chargingTime, b.dischargingTime];
  setTimeout(() => r({fired, moved: JSON.stringify(first) !==
      JSON.stringify([b.charging, b.level, b.chargingTime, b.dischargingTime])}), 20000);
})
"""


def launch(config):
    d = tempfile.mkdtemp(prefix="fury-0121-")
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
    t = ws.call("Target.createTarget", {"url": "https://example.com"})["targetId"]
    s = ws.call("Target.attachToTarget", {"targetId": t, "flatten": True})["sessionId"]
    ws.call("Runtime.enable", session=s)
    time.sleep(2.5)
    return p, ws, s, d


def js(ws, s, expr, timeout=40000):
    r = ws.call("Runtime.evaluate",
                {"expression": expr, "awaitPromise": True, "returnByValue": True,
                 "timeout": timeout}, session=s)
    if "exceptionDetails" in r:
        return {"threw": str(r["exceptionDetails"].get("text"))}
    return r["result"].get("value")


def stop(p, d):
    p.terminate()
    try:
        p.wait(timeout=12)
    except subprocess.TimeoutExpired:
        p.kill()
    shutil.rmtree(d, ignore_errors=True)


MAINS = {"schema_version": 1,
         "battery": {"charging": True, "level": 1.0,
                     "chargingTime": 0.0, "dischargingTime": -1.0}}

print("\n--- the host's real battery, with nothing configured ---")
p, ws, s, d = launch({"schema_version": 1})
host = js(ws, s, READ)
print(f"  host reports {json.dumps(host)}")
stop(p, d)
# The whole test is worthless on a machine that is already at 100% on mains,
# because the configured tuple and the real one would be the same.
meaningful = not (host.get("charging") is True and host.get("level") == 1.0)
check(meaningful,
      "this host's real battery differs from the tuple under test, so a pass "
      "cannot be a coincidence"
      if meaningful else
      "THIS HOST IS AT 100% ON MAINS — unplug it and run again, every check "
      "below would pass for the wrong reason")

print("\n--- with battery configured ---")
p, ws, s, d = launch(MAINS)
got = js(ws, s, READ)
print(f"  browser reports {json.dumps(got)}")
check(got.get("charging") is True and got.get("level") == 1.0
      and got.get("chargingTime") == 0 and got.get("dischargingTime") == "Infinity",
      f"all four values are the configured ones: {json.dumps(got)}")
check(got.get("native"),
      "navigator.getBattery is still [native code] — values substituted, not "
      "the binding replaced")
check(isinstance(got.get("level"), (int, float))
      and abs(got["level"] * 100 - round(got["level"] * 100)) < 1e-9,
      "level went through EnsureTwoSignificantDigits rather than around it")
# The invariants the mac and windows backends can actually produce. Checked as
# invariants rather than against the config, so a future persona that sets a
# different tuple is checked too.
check(not (got.get("charging") is True and got.get("dischargingTime") != "Infinity"),
      "charging implies dischargingTime is Infinity — a state a real backend can reach")

frame = js(ws, s, IN_IFRAME)
check(isinstance(frame, dict)
      and frame.get("level") == got.get("level")
      and frame.get("charging") == got.get("charging")
      and frame.get("chargingTime") == got.get("chargingTime")
      and frame.get("dischargingTime") == got.get("dischargingTime"),
      f"a nested document reports the same battery, not the host's: {json.dumps(frame)}")

print("  watching for events for 20s...")
watched = js(ws, s, WATCH, timeout=40000)
check(isinstance(watched, dict) and not watched.get("fired") and not watched.get("moved"),
      f"no battery event fires and no value moves: {json.dumps(watched)} — this is "
      f"the check that tells the dispatcher seam from the getter seam, where the "
      f"values would sit still while events kept arriving off the host's cell")
stop(p, d)

bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
