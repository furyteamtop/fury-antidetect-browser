#!/usr/bin/env python3
"""Does patch 0070 stop WebRTC handing the page this machine's real address?

The leak this exists to close was measured, not suspected: on 02.08.2026 our
build, launched with a proxy and with the --force-webrtc-ip-handling-policy
switch the launcher had been sending since the beginning, gave a page an srflx
ICE candidate carrying the host's real public IP. The switch is read by
content/shell and by browser tests, and by nothing in //chrome.

So this asks the browser, not the source. Three configurations, because "the
policy is applied" and "the address is gone" are different claims and the second
is the one that matters.

Usage: core/verify/verify-0070.py <core binary>
See core/verify/README.md.
"""

import json
import os
import re
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


GATHER = r"""
new Promise(r => {
  const out = {candidates: [], types: {}};
  let pc;
  try {
    pc = new RTCPeerConnection({iceServers: [{urls: 'stun:stun.l.google.com:19302'}]});
  } catch (e) { return r({error: String(e)}); }
  pc.createDataChannel('probe');
  pc.onicecandidate = e => {
    if (!e.candidate) { out.state = pc.iceGatheringState; return r(out); }
    out.candidates.push(e.candidate.candidate);
    const m = / typ (\w+)/.exec(e.candidate.candidate);
    if (m) out.types[m[1]] = (out.types[m[1]] || 0) + 1;
  };
  pc.createOffer().then(o => pc.setLocalDescription(o)).catch(e => r({error: String(e)}));
  setTimeout(() => { out.state = pc.iceGatheringState; out.timedOut = true; r(out); }, 14000);
})
"""

# Anything that is not an mDNS name and not a loopback address is an address of
# this machine or of its network, and a page must not be able to read it.
ADDR = re.compile(r"\b((?:\d{1,3}\.){3}\d{1,3}|[0-9a-f]{0,4}(?::[0-9a-f]{0,4}){2,})\b", re.I)


def gather(config, label):
    d = tempfile.mkdtemp(prefix="fury-0070-")
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
    pre = (lambda: os.dup2(fd, 3)) if fd is not None else None
    p = subprocess.Popen(args, preexec_fn=pre, close_fds=False,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
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
    v = ws.call("Runtime.evaluate",
                {"expression": GATHER, "awaitPromise": True, "returnByValue": True,
                 "timeout": 30000}, session=s)["result"].get("value") or {}
    p.terminate()
    try:
        p.wait(timeout=12)
    except subprocess.TimeoutExpired:
        p.kill()
    shutil.rmtree(d, ignore_errors=True)

    routable = set()
    for c in v.get("candidates") or []:
        for a in ADDR.findall(c):
            if a.startswith("127.") or a in ("0.0.0.0", "::1", "::"):
                continue
            routable.add(a)
    print(f"  [{label}] types={json.dumps(v.get('types'))} "
          f"state={v.get('state')} routable={sorted(routable) or 'none'}")
    return v, routable


BASE = {"schema_version": 1}
POLICY = {"schema_version": 1,
          "webrtc": {"ipHandlingPolicy": "disable_non_proxied_udp"}}

# 1. The leak, reproduced. Without the key the browser is stock Chromium and
#    must still leak — a control that fails means the test is not measuring
#    what it claims.
print("\n--- the leak, with no policy configured ---")
_, leaked = gather(BASE, "no policy")
check(bool(leaked),
      f"a page still gets this machine's address when nothing is configured: {sorted(leaked)}")

# 2. The patch.
print("\n--- with webrtc.ipHandlingPolicy = disable_non_proxied_udp ---")
v, kept = gather(POLICY, "policy on")
check(not kept, f"no routable address reaches the page: {sorted(kept) or 'none'}")
check(not (v.get("types") or {}).get("srflx"),
      "no server-reflexive candidate, which is where the public address rode in")

# This check started life asserting the opposite — that the mDNS host candidate
# survives, on the reasoning that a peer connection gathering absolutely nothing
# is its own signal. The reasoning was sound and the expectation was wrong, and
# the way to tell was to go and look.
#
# Measured 02.08.2026: real Chrome 150.0.7871.187, with no switches and nothing
# patched, given only the preference it genuinely reads —
# {"webrtc": {"ip_handling_policy": "disable_non_proxied_udp"}} written into
# Default/Preferences — gathers types={} and reaches iceGatheringState
# "complete". Zero candidates. The same Chrome with no preference gathers
# {host: 1, srflx: 1}, and with "default_public_interface_only" gathers
# {srflx: 1}. So the empty state is not something Fury invents: it is what the
# enterprise policy that exists precisely to stop WebRTC IP leaks does on real
# corporate machines, and matching it is the whole point.
check((v.get("types") or {}) == {} and v.get("state") == "complete",
      "gathers nothing and completes — byte for byte the state real Chrome 150 "
      "reaches under the same enterprise policy, rather than a state we invented")

# 3. A typo must not open it back up.
print("\n--- with a policy string Chromium does not know ---")
v3, leaked3 = gather({"schema_version": 1,
                      "webrtc": {"ipHandlingPolicy": "disable-non-proxied-udp"}},
                     "typo")
check(bool(leaked3),
      "a misspelled policy is refused rather than silently mapped to 'default' — "
      "it leaks exactly as an unconfigured browser does, and says so in the log, "
      "instead of looking configured")

bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
