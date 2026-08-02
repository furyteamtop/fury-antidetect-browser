#!/usr/bin/env python3
"""Does --fury-lock-devtools actually refuse anything?

Until patch 0302 it did not. agent/src/launcher.rs pushed the switch on every
restricted launch and nothing in core/src read it — grep over the whole tree
returned hits only in the Rust that writes it and the Rust test that asserts it
was written. The RBAC restriction that docs/04 insists must be technical rather
than a hidden button was a hidden button, and an operator forbidden from reading
a profile's cookies could open DevTools and read them.

The control matters as much as the check: without the switch, a CDP attach MUST
still succeed, or this is measuring a browser that refuses everybody.

Note the switch string is written independently in two languages —
agent/src/launcher.rs and components/fury/fury_switches.cc — and nothing in
either checks the other. This spells it the way the Rust does, on purpose: a
drifted name would otherwise fail open, silently, forever.

Usage: core/verify/verify-0302.py <core binary>
See core/verify/README.md.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cdp import WS  # noqa: E402

CORE = sys.argv[1]
# Exactly the string agent/src/launcher.rs pushes.
LOCK = "--fury-lock-devtools"
results = []


def check(ok, text):
    results.append((ok, text))
    print(f"  {'OK  ' if ok else 'FAIL'} {text}", flush=True)


def launch(extra, prefs=None):
    d = tempfile.mkdtemp(prefix="fury-0302-")
    if prefs is not None:
        os.makedirs(os.path.join(d, "Default"), exist_ok=True)
        with open(os.path.join(d, "Default", "Preferences"), "w") as f:
            json.dump(prefs, f)
    p = subprocess.Popen(
        [CORE, f"--user-data-dir={d}", "--remote-debugging-port=0",
         "--no-first-run", "--no-default-browser-check",
         "--window-position=-4000,-4000", "--window-size=600,400"] + extra,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
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
    return p, port, d


def stop(p, d):
    p.terminate()
    try:
        p.wait(timeout=12)
    except subprocess.TimeoutExpired:
        p.kill()
    shutil.rmtree(d, ignore_errors=True)


def attach_and_evaluate(port):
    """Open a page and try to run script in it over CDP.

    Attaching to the BROWSER target is not the test — the browser endpoint is
    not a debuggable page. What the restriction has to refuse is inspecting a
    TAB, which is where the cookies are.
    """
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version",
                                timeout=15) as r:
        ws = WS(json.load(r)["webSocketDebuggerUrl"])
    target = ws.call("Target.createTarget", {"url": "https://example.com"})["targetId"]
    try:
        session = ws.call("Target.attachToTarget",
                          {"targetId": target, "flatten": True})["sessionId"]
    except RuntimeError as e:
        return {"attached": False, "error": str(e)}
    # createTarget returns before the navigation lands, so the first evaluate
    # sees about:blank. The control is "script runs in a real page", not "script
    # runs" — checking too early made the control fail and would have made this
    # whole script look like a browser that refuses everybody.
    time.sleep(3)
    try:
        r = ws.call("Runtime.evaluate",
                    {"expression": "document.location.href", "returnByValue": True},
                    session=session)
        return {"attached": True, "value": r["result"].get("value")}
    except RuntimeError as e:
        return {"attached": True, "evaluated": False, "error": str(e)}


print("\n--- the control: no switch, CDP must work ---")
p, port, d = launch([])
free = attach_and_evaluate(port)
print(f"  {json.dumps(free)[:200]}")
stop(p, d)
check(free.get("attached") and free.get("value", "").startswith("http"),
      "an unrestricted profile still attaches and evaluates — otherwise every "
      "check below would pass on a browser that simply refuses everyone")

print(f"\n--- with {LOCK} ---")
p, port, d = launch([LOCK])
locked = attach_and_evaluate(port)
print(f"  {json.dumps(locked)[:260]}")
check(not (locked.get("attached") and locked.get("value")),
      f"a locked profile cannot be inspected over CDP: {json.dumps(locked)[:180]}")

# The bypass this patch's FIRST version left open, and the reason there are five
# guards rather than two.
#
# IsInspectionAllowed(profile, extension=nullptr) hits an arm that, on
# kDisallowed, reads prefs::kDeveloperToolsAvailabilityAllowlist STRAIGHT off
# the profile and returns true when it is non-empty — not through the policy
# checker, so guarding GetEffectiveAvailability and GetDevToolsAvailabilityForUrl
# left it wide open. That preference lives in the Preferences JSON inside the
# user-data-dir, which is exactly the file a restricted operator has a copy of.
# One line in it and DevTools came back.
print("\n--- with the lock AND an allowlist written into the profile ---")
p2, port2, d2 = launch([LOCK], prefs={"devtools": {"availability_allowlist": ["*"]}})
bypass = attach_and_evaluate(port2)
print(f"  {json.dumps(bypass)[:260]}")
check(not (bypass.get("attached") and bypass.get("value")),
      "editing the profile's own Preferences does not unlock it — the guard is "
      "read from argv, which the operator cannot change after the browser starts")
stop(p2, d2)

# The remote-debugging listener may still answer /json/version — the refusal is
# per-attach, in DevToolsAgentHostImpl::AttachInternal, not at the socket. Which
# is the right layer: one guard then covers --remote-debugging-port,
# --remote-debugging-pipe and chrome.debugger alike. Recorded rather than
# asserted either way, because a future patch could reasonably close the port
# too and this check should not pretend that is a regression.
try:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=10) as r:
        listed = json.load(r)
    print(f"  (the endpoint still lists {len(listed)} target(s); the refusal is "
          f"per-attach, not at the socket)")
except (urllib.error.URLError, OSError) as e:
    print(f"  (the endpoint itself is closed: {e})")
stop(p, d)

bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
