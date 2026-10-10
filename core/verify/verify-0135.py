#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Does patch 0135 give a phone persona the motion sensors of a phone in a hand?

A phone held by a person reports devicemotion about sixty times a second:
gravity tilted by how it is held, a tremor of hundredths of m/s^2, a rotation
rate of a few degrees per second, and deviceorientation angles that agree
with the gravity it reports. A desktop reports nothing; on a phone persona
that is a contradiction any page reads in a few seconds.

The claims: the events arrive at a phone's rate; the gravity they carry is
9.8 m/s^2; the orientation angles are the ones that gravity implies (both
come from one model, and the device service derives the angles from the
quaternion, so a sign error between the two would show here); the hand moves
a little and not a lot; the Generic Sensor API reads the same; one profile
holds its phone the same way on two launches and another profile its own
way; and a desktop config still reports nothing.

Usage: core/verify/verify-0135.py <core binary>
See core/verify/README.md.
"""

import http.server
import json
import math
import os
import shutil
import signal
import socketserver
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cdp import browser  # noqa: E402

CORE = sys.argv[1]
results = []
exits = []


def check(ok, text):
    results.append((ok, text))
    print(f"  {'OK  ' if ok else 'FAIL'} {text}", flush=True)


PAGE = """<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1"><title>m</title>
<script>
window.out = null;
(async () => {
  const st = () => ({ n: 0, s: 0, q: 0 });
  const add = (a, v) => { if (typeof v === 'number' && isFinite(v)) { a.n++; a.s += v; a.q += v * v; } };
  const fin = (a) => a.n ? { mean: a.s / a.n, sd: Math.sqrt(Math.max(0, a.q / a.n - (a.s / a.n) ** 2)) } : null;
  const m = { n: 0, iv: [], gx: st(), gy: st(), gz: st(), g: st(), lx: st(), ly: st(), lz: st(), ra: st(), rb: st(), rg: st() };
  const o = { n: 0, b: st(), c: st(), a: st() };
  addEventListener('devicemotion', (e) => {
    m.n++; m.iv.push(e.interval);
    const g = e.accelerationIncludingGravity, l = e.acceleration, r = e.rotationRate;
    if (g && g.x !== null) { add(m.gx, g.x); add(m.gy, g.y); add(m.gz, g.z); add(m.g, Math.hypot(g.x, g.y, g.z)); }
    if (l && l.x !== null) { add(m.lx, l.x); add(m.ly, l.y); add(m.lz, l.z); }
    if (r && r.alpha !== null) { add(m.ra, r.alpha); add(m.rb, r.beta); add(m.rg, r.gamma); }
  });
  addEventListener('deviceorientation', (e) => { o.n++; add(o.b, e.beta); add(o.c, e.gamma); add(o.a, e.alpha); });
  let acc = { started: false, readings: 0, g: st(), error: null };
  try {
    const s = new Accelerometer({ frequency: 60 });
    s.onreading = () => { acc.readings++; add(acc.g, Math.hypot(s.x, s.y, s.z)); };
    s.onerror = (e) => { acc.error = e.error && e.error.name; };
    s.onactivate = () => { acc.started = true; };
    s.start();
  } catch (e) { acc.error = String(e && e.name); }
  await new Promise((r) => setTimeout(r, 3000));
  const iv = m.iv.sort((a, b) => a - b);
  window.out = {
    motion: { n: m.n, interval: iv.length ? iv[iv.length >> 1] : null,
      g: fin(m.g), gx: fin(m.gx), gy: fin(m.gy), gz: fin(m.gz),
      lx: fin(m.lx), ly: fin(m.ly), lz: fin(m.lz), ra: fin(m.ra), rb: fin(m.rb), rg: fin(m.rg) },
    orient: { n: o.n, beta: fin(o.b), gamma: fin(o.c), alpha: fin(o.a) },
    accel: { started: acc.started, readings: acc.readings, g: fin(acc.g), error: acc.error },
  };
})();
</script>"""


class Pages(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        data = PAGE.encode()
        self.send_response(200)
        self.send_header("content-type", "text/html")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Pages)
threading.Thread(target=server.serve_forever, daemon=True).start()
URL = f"http://127.0.0.1:{server.server_address[1]}/"


def run(config):
    d = tempfile.mkdtemp(prefix="fury-0135-")
    cfg = os.path.join(d, "config.json")
    with open(cfg, "w") as f:
        json.dump(config, f)
    fd = os.open(cfg, os.O_RDONLY)
    os.set_inheritable(fd, True)
    p = subprocess.Popen([CORE, f"--user-data-dir={d}", "--remote-debugging-port=0",
                          "--no-first-run", "--no-default-browser-check", "--use-mock-keychain",
                          "--fury-fp-fd=3"],
                         preexec_fn=lambda: os.dup2(fd, 3), close_fds=False,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    os.close(fd)
    try:
        port = None
        for _ in range(300):
            try:
                port = int(open(os.path.join(d, "DevToolsActivePort")).readline())
                break
            except (OSError, ValueError):
                time.sleep(0.2)
        time.sleep(2)
        ws = browser(port)
        # In front before the page loads: a hidden tab gets no motion, on a
        # phone and here (FuryPhoneMotion::Tick), and a window this Mac opened
        # behind another counted only the seconds it was visible.
        t = ws.call("Target.createTarget", {"url": "about:blank"})["targetId"]
        s = ws.call("Target.attachToTarget", {"targetId": t, "flatten": True})["sessionId"]
        ws.call("Page.enable", session=s)
        ws.call("Page.bringToFront", session=s)
        time.sleep(0.5)
        ws.call("Page.navigate", {"url": URL}, session=s)
        ws.call("Runtime.enable", session=s)
        v = None
        for _ in range(40):
            time.sleep(0.25)
            v = ws.call("Runtime.evaluate", {"expression": "window.out", "returnByValue": True},
                        session=s)["result"].get("value")
            if v:
                break
        # The sensors' lifetime: a browser page in the same tab drops them, a
        # web page brings them back, and closing the tab and then the browser
        # must not crash (they used to outlive their provider on quit).
        ws.call("Page.navigate", {"url": "chrome://version"}, session=s)
        time.sleep(1)
        ws.call("Page.navigate", {"url": URL}, session=s)
        time.sleep(1)
        ws.call("Target.closeTarget", {"targetId": t})
        time.sleep(0.5)
        return v
    finally:
        p.send_signal(signal.SIGHUP)
        try:
            p.wait(timeout=12)
        except subprocess.TimeoutExpired:
            p.kill()
        exits.append(p.returncode)
        shutil.rmtree(d, ignore_errors=True)


def phone(seed):
    return {"schema_version": 1, "navigator": {"maxTouchPoints": 5},
            "screen": {"width": 384, "height": 832, "availWidth": 384, "availHeight": 832,
                       "devicePixelRatio": 2.8125, "chromeHeightDelta": 192, "colorDepth": 24},
            "mobile": {"enabled": True, "connectionType": "cellular", "touchSeed": seed}}


def held(o):
    """beta and gamma that the reported gravity implies (W3C: g = (-cb sg, sb, cb cg) g)."""
    gx, gy, gz = o["motion"]["gx"]["mean"], o["motion"]["gy"]["mean"], o["motion"]["gz"]["mean"]
    g = math.sqrt(gx * gx + gy * gy + gz * gz)
    return math.degrees(math.asin(gy / g)), math.degrees(math.atan2(-gx, gz))


desk = run({"schema_version": 1})
a = run(phone(1))
a2 = run(phone(1))
b = run(phone(2))
for name, o in (("desktop", desk), ("phone A", a), ("phone A again", a2), ("phone B", b)):
    print(f"  {name}: {json.dumps(o)}")

m = a["motion"]
check(m["n"] >= 140 and m["interval"] is not None and 14 <= m["interval"] <= 20,
      f"phone: devicemotion arrives at a phone's rate ({m['n']} in 3 s, interval {m['interval']} ms)")
check(m["g"] and abs(m["g"]["mean"] - 9.81) < 0.25,
      f"phone: the acceleration it reports includes gravity, |g| = {m['g'] and round(m['g']['mean'], 3)} m/s^2")
beta_g, gamma_g = held(a)
ob, og = a["orient"]["beta"], a["orient"]["gamma"]
check(a["orient"]["n"] >= 100 and ob and og
      and abs(ob["mean"] - beta_g) < 3 and abs(og["mean"] - gamma_g) < 3,
      f"phone: deviceorientation agrees with that gravity: beta {ob and round(ob['mean'], 1)} "
      f"vs {round(beta_g, 1)}, gamma {og and round(og['mean'], 1)} vs {round(gamma_g, 1)} "
      f"({a['orient']['n']} events)")
check(20 <= beta_g <= 85,
      f"phone: held upright and tilted back, as a phone is read (beta {round(beta_g, 1)})")
lin = [m[k]["sd"] for k in ("lx", "ly", "lz") if m[k]]
check(len(lin) == 3 and all(0.004 < x < 0.4 for x in lin),
      f"phone: the hand moves a little: linear acceleration spread {[round(x, 3) for x in lin]} m/s^2")
rot = [m[k]["sd"] for k in ("ra", "rb", "rg") if m[k]]
check(len(rot) == 3 and max(rot) > 0.2 and max(rot) < 30,
      f"phone: and turns a little: rotation rate spread {[round(x, 2) for x in rot]} deg/s")
acc = a["accel"]
check(acc["started"] and acc["readings"] >= 100 and acc["g"] and abs(acc["g"]["mean"] - 9.81) < 0.25,
      f"phone: the Generic Sensor Accelerometer reads the same ({acc['readings']} readings, "
      f"|g| {acc['g'] and round(acc['g']['mean'], 3)}, error {acc['error']})")
beta_a2, gamma_a2 = held(a2)
beta_b, gamma_b = held(b)
check(abs(beta_g - beta_a2) < 4 and abs(gamma_g - gamma_a2) < 4,
      f"the same profile holds its phone the same way on another launch "
      f"({round(beta_g, 1)}/{round(gamma_g, 1)} and {round(beta_a2, 1)}/{round(gamma_a2, 1)})")
check(math.hypot(beta_g - beta_b, gamma_g - gamma_b) > 4,
      f"and another profile its own way ({round(beta_b, 1)}/{round(gamma_b, 1)})")
# Desktop Chrome with no sensors fires one devicemotion and one
# deviceorientation with every value null; that is upstream and stays.
check(desk["motion"]["g"] is None and desk["orient"]["beta"] is None and not desk["accel"]["started"],
      f"desktop: no readings at all, as before ({desk['motion']['n']} empty motion event(s), "
      f"Accelerometer {desk['accel']['error']})")
check(all(code == 0 for code in exits),
      f"every core exited cleanly after the tab went through a browser page and closed "
      f"(exit codes {exits}; a crash is a negative signal number)")

server.shutdown()
bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
