#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Does patch 0132 make the mouse a finger on a phone persona?

Patch 0130 tells the page it is on a touchscreen (touch events, a coarse
pointer, no hover). Without 0132 that touchscreen only ever sent mouse events,
which no phone does. Every check runs under a desktop config too and must come
out the other way, so the phone run cannot pass by the host agreeing.

The input is CDP's Input.dispatchMouseEvent, which is also how Fury's agent
clicks (MCP), so this covers automation as well as a person's mouse.

Usage: core/verify/verify-0132.py <core binary>
See core/verify/README.md.
"""

import http.server
import json
import os
import shutil
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


def check(ok, text):
    results.append((ok, text))
    print(f"  {'OK  ' if ok else 'FAIL'} {text}", flush=True)


PAGE = """<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1">
<title>t</title>
<style>body{margin:0;font:16px sans-serif} #b{position:absolute;left:40px;top:60px;width:200px;height:80px}
#h{position:absolute;left:40px;top:200px;width:200px;height:80px;background:#ddd} .tall{height:4000px}</style>
<button id="b">tap</button><div id="h">hover</div><div class="tall"></div>
<script>
window.log = [];
for (const t of ['touchstart','touchmove','touchend','mousedown','mouseup','mousemove','mouseover','click','pointerdown','pointermove','wheel'])
  addEventListener(t, e => log.push(t + (e.pointerType ? ':' + e.pointerType : '')), {passive: true, capture: true});
</script>"""


class Pages(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        data = PAGE.encode() if self.path == "/" else TAPS.encode() if self.path == "/taps" else b""
        self.send_response(200 if data else 404)
        self.send_header("content-type", "text/html")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Pages)
threading.Thread(target=server.serve_forever, daemon=True).start()
URL = f"http://127.0.0.1:{server.server_address[1]}/"


def launch(config):
    d = tempfile.mkdtemp(prefix="fury-0132-")
    args = [CORE, f"--user-data-dir={d}", "--remote-debugging-port=0",
            "--no-first-run", "--no-default-browser-check", "--use-mock-keychain",
            "--window-position=0,0", "--window-size=700,900", "--fury-fp-fd=3"]
    cfg = os.path.join(d, "config.json")
    with open(cfg, "w") as f:
        json.dump(config, f)
    fd = os.open(cfg, os.O_RDONLY)
    os.set_inheritable(fd, True)
    p = subprocess.Popen(args, preexec_fn=lambda: os.dup2(fd, 3), close_fds=False,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
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
    return p, browser(port), d


def stop(p, d):
    p.terminate()
    try:
        p.wait(timeout=12)
    except subprocess.TimeoutExpired:
        p.kill()
    shutil.rmtree(d, ignore_errors=True)


def measure(config):
    p, ws, d = launch(config)
    try:
        t = ws.call("Target.createTarget", {"url": URL})["targetId"]
        s = ws.call("Target.attachToTarget", {"targetId": t, "flatten": True})["sessionId"]
        ws.call("Runtime.enable", session=s)
        time.sleep(2.5)

        def js(expr):
            r = ws.call("Runtime.evaluate", {"expression": expr, "returnByValue": True}, session=s)
            return r["result"].get("value")

        def mouse(kind, x, y, button="none", buttons=0):
            ws.call("Input.dispatchMouseEvent", {"type": kind, "x": x, "y": y, "button": button,
                                                 "buttons": buttons, "clickCount": 1 if kind != "mouseMoved" else 0},
                    session=s)

        # A click on the button.
        js("log.length = 0")
        mouse("mousePressed", 140, 100, "left", 1)
        mouse("mouseReleased", 140, 100, "left", 0)
        time.sleep(0.6)
        click = js("log.slice()")

        # Hovering over the grey box with no button down.
        js("log.length = 0")
        for y in (210, 230, 250, 270):
            mouse("mouseMoved", 140, y)
            time.sleep(0.05)
        time.sleep(0.3)
        hover = js("log.slice()")

        # A drag upwards over the page: a finger scrolls, a mouse selects.
        js("log.length = 0; scrollTo(0, 0)")
        mouse("mousePressed", 300, 700, "left", 1)
        for y in range(680, 300, -40):
            mouse("mouseMoved", 300, y, "left", 1)
            time.sleep(0.02)
        mouse("mouseReleased", 300, 300, "left", 0)
        time.sleep(1.2)
        drag = js("({log: log.slice(), y: scrollY})")

        # The wheel, five notches down.
        js("log.length = 0; scrollTo(0, 0)")
        mouse("mouseMoved", 300, 500)
        for _ in range(5):
            ws.call("Input.dispatchMouseEvent", {"type": "mouseWheel", "x": 300, "y": 500,
                                                 "deltaX": 0, "deltaY": 100}, session=s)
            time.sleep(0.03)
        time.sleep(1.5)
        wheel = js("({log: log.slice(), y: scrollY})")
        return click, hover, drag, wheel
    finally:
        stop(p, d)


DESKTOP = {"schema_version": 1}
PHONE = {"schema_version": 1,
         "navigator": {"maxTouchPoints": 5},
         "screen": {"width": 384, "height": 832, "availWidth": 384, "availHeight": 832,
                    "devicePixelRatio": 2.8125, "chromeHeightDelta": 192, "colorDepth": 24},
         "mobile": {"enabled": True, "connectionType": "cellular", "touchSeed": 12345}}

TAPS = """<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1">
<title>taps</title><style>body{margin:0} #t{position:absolute;left:195px;top:295px;width:10px;height:10px;padding:0;border:0}</style>
<button id="t"></button>
<script>
window.taps = []; window.hits = 0;
document.getElementById('t').addEventListener('click', () => hits++);
addEventListener('touchstart', e => { const t = e.touches[0];
  taps.push([t.clientX, t.clientY, t.radiusX, t.radiusY, t.force, t.rotationAngle]); }, {passive: true});
</script>"""


def measure_taps(config, n=20):
    """n taps at the centre of a 10x10 button at (200, 300)."""
    p, ws, d = launch(config)
    try:
        url = URL + "taps"
        t = ws.call("Target.createTarget", {"url": url})["targetId"]
        s = ws.call("Target.attachToTarget", {"targetId": t, "flatten": True})["sessionId"]
        ws.call("Runtime.enable", session=s)
        time.sleep(2.5)
        for _ in range(n):
            for kind, b in (("mousePressed", 1), ("mouseReleased", 0)):
                ws.call("Input.dispatchMouseEvent", {"type": kind, "x": 200, "y": 300, "button": "left",
                                                     "buttons": b, "clickCount": 1}, session=s)
            time.sleep(0.35)
        time.sleep(0.5)
        r = ws.call("Runtime.evaluate", {"expression": "({taps, hits})", "returnByValue": True},
                    session=s)["result"]["value"]
        return r["taps"], r["hits"]
    finally:
        stop(p, d)


print("\n--- desktop config ---")
d_click, d_hover, d_drag, d_wheel = measure(DESKTOP)
print(f"  click {d_click}\n  hover {d_hover}\n  drag scrollY {d_drag['y']}, {len(d_drag['log'])} events")
print("\n--- phone config ---")
click, hover, drag, wheel = measure(PHONE)
print(f"  click {click}\n  hover {hover}\n  drag scrollY {drag['y']}, events {sorted(set(drag['log']))}")
print(f"  wheel scrollY {wheel['y']} (desktop {d_wheel['y']}), events {sorted(set(wheel['log']))} (desktop {sorted(set(d_wheel['log']))})")

check("touchstart" not in d_click and "pointerdown:mouse" in d_click,
      "desktop: a click is a mouse, so what follows is the patch")
# A tap on Android Chrome: pointerdown, touchstart, touchend, then the
# compatibility mouse events and a click that is a PointerEvent of type touch.
check("touchstart" in click and "touchend" in click and "click:touch" in click
      and click.index("touchstart") < click.index("touchend") < click.index("click:touch"),
      f"phone: a click reaches the page as touchstart, touchend, then a click of pointerType touch")
check("pointerdown:touch" in click and "pointerdown:mouse" not in click,
      "and its pointer events say pointerType touch, not mouse")
check(any(e.startswith(("mousemove", "pointermove")) for e in d_hover),
      "desktop: moving the mouse over the page is seen")
check(not any(e.startswith(("mousemove", "mouseover", "pointermove")) for e in hover),
      f"phone: a mouse moving with no button down reaches the page as nothing, a phone cannot hover ({hover})")
check(d_drag["y"] == 0, f"desktop: a drag does not scroll ({d_drag['y']})")
check(drag["y"] > 100 and "touchmove" in drag["log"],
      f"phone: a drag is a finger scrolling the page (scrollY {drag['y']}, touchmove seen)")

check("wheel" in d_wheel["log"] and d_wheel["y"] > 0,
      f"desktop: the wheel arrives as wheel events and scrolls ({d_wheel['y']})")
check("wheel" not in wheel["log"] and "touchmove" in wheel["log"] and wheel["y"] > 100,
      f"phone: the wheel is a finger, no wheel event reaches the page and it still scrolls "
      f"(scrollY {wheel['y']})")

print("\n--- the finger: 20 taps on a 10x10 button ---")
taps, hits = measure_taps(PHONE)
taps2, _ = measure_taps(PHONE)
other = dict(PHONE, mobile=dict(PHONE["mobile"], touchSeed=987654))
taps3, _ = measure_taps(other)
mean = lambda ts, i: sum(t[i] for t in ts) / len(ts)
xs = sorted({(round(t[0], 1), round(t[1], 1)) for t in taps})
m1 = (mean(taps, 0) - 200, mean(taps, 1) - 300)
m2 = (mean(taps2, 0) - 200, mean(taps2, 1) - 300)
m3 = (mean(taps3, 0) - 200, mean(taps3, 1) - 300)
print(f"  {len(taps)} touches, {len(xs)} distinct points, hits {hits}")
print(f"  mean offset: profile A {m1[0]:+.2f},{m1[1]:+.2f}; again {m2[0]:+.2f},{m2[1]:+.2f}; profile B {m3[0]:+.2f},{m3[1]:+.2f}")
print(f"  radius {min(t[2] for t in taps):.1f}..{max(t[2] for t in taps):.1f}, "
      f"force {min(t[4] for t in taps):.2f}..{max(t[4] for t in taps):.2f}, "
      f"angle {min(t[5] for t in taps):.0f}..{max(t[5] for t in taps):.0f}")
check(len(taps) == 20 and len(xs) >= 10,
      f"20 taps at one point land on {len(xs)} different points, not the cursor's pixel each time")
check(all(abs(t[0] - 200) <= 7 and abs(t[1] - 300) <= 7 for t in taps),
      "and all within 7 px of it: a bias of a few px plus a scatter of at most 4")
check(all(5 <= t[2] <= 20 and 5 <= t[3] <= 20 for t in taps) and len({round(t[2], 2) for t in taps}) > 5,
      "the contact is a fingertip's size and changes from touch to touch")
check(all(0 < t[4] < 1 for t in taps) and len({round(t[4], 3) for t in taps}) > 5,
      "the force is never exactly 1, and varies")
check(abs(m1[0] - m2[0]) < 1.2 and abs(m1[1] - m2[1]) < 1.2,
      f"the same profile's finger lands the same way on another launch")
check(abs(m1[0] - m3[0]) + abs(m1[1] - m3[1]) > 0.8,
      f"and another profile's lands its own way")
check(hits == 20, f"a 10x10 button still takes all 20 taps ({hits}): Blink adjusts a tap with a contact area")

server.shutdown()
bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
