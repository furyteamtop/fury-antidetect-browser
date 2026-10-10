#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Does the host screen's device pixel ratio show through a persona's?

window.devicePixelRatio is only the number a page asks for first. The ratio
also decides where a thin border rounds to, devicePixelContentBoxSize, which
srcset candidate loads, the resolution media queries, and what a CSS paint
worklet reads, in the page and in a cross-site frame. Measured on 10.10.2026
on a 2x Mac: with a 2.8125 phone and with 1x and 1.25x desktop personas the
border, devicePixelContentBoxSize, srcset, a cross-site frame's border and the
paint worklet all followed the host's 2. Fixed in 0131 (the compositor at the
persona's ratio, in the page and in its cross-site frames) and 0020 (the
worklet).

The desktop personas must lose nothing for it: the window, the page's sizes,
the screen and its available area are the host run's, the page still fills
the view on screen, and a click still lands.

The paint worklet cannot be read from script; it is read off a screenshot,
by how wide the black bar it drew at devicePixelRatio * 20 px came out.

Usage: core/verify/verify-0131-dpr.py <core binary>
See core/verify/README.md.
"""

import base64
import http.server
import json
import os
import shutil
import signal
import socketserver
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cdp import browser  # noqa: E402

CORE = sys.argv[1]

PAGE = """<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1"><title>dpr</title>
<style>body{margin:0;background:#fff} #p{position:absolute;left:10px;top:10px;width:200px;height:20px;background:paint(dpr)}
#b{position:absolute;left:10px;top:60px;width:50px;height:10px;border-top:0.3px solid #000} #r{position:absolute;left:10px;top:100px;width:100px;height:100px}</style>
<div id="p"></div><div id="b"></div><div id="r"></div>
<img id="i" srcset="/img1 1x, /img2 2x, /img3 3x" style="position:absolute;top:220px">
<div id="corner" style="position:fixed;right:0;bottom:0;width:30px;height:30px;background:#f00"></div>
<button id="btn" style="position:absolute;left:300px;top:300px;width:80px;height:40px" onclick="window.hits=(window.hits||0)+1">b</button>
<iframe id="f" src="http://localhost:%d/frame" style="position:absolute;top:260px"></iframe>
<script>
CSS.paintWorklet.addModule('/paint.js');
window.out = {};
const mq = q => matchMedia(q).matches;
out.dpr = devicePixelRatio;
out.res2812 = mq('(resolution: 2.8125dppx)'); out.res2 = mq('(resolution: 2dppx)');
out.webkit = mq('(-webkit-device-pixel-ratio: 2.8125)');
out.border = document.getElementById('b').getBoundingClientRect().height - 10;
new ResizeObserver(e => { const s = e[0].devicePixelContentBoxSize; out.dpcb = s ? s[0].inlineSize : null; })
  .observe(document.getElementById('r'), {box: 'device-pixel-content-box'});
out.isExtended = screen.isExtended;
Object.assign(out, {iw: innerWidth, ih: innerHeight, ow: outerWidth, oh: outerHeight, sx: screenX, sy: screenY, sw: screen.width, sh: screen.height, saw: screen.availWidth, sah: screen.availHeight, cw: document.documentElement.clientWidth, dw: matchMedia(`(device-width: ${screen.width}px)`).matches});
out.hasScreenDetails = 'getScreenDetails' in window;
addEventListener('message', e => (out.frames = out.frames || []).push(e.data));
setTimeout(() => { out.img = document.getElementById('i').currentSrc.split('/').pop(); }, 800);
</script>"""
# The frame measures as soon as its script runs, then again later: before
# 0131 handed the child the persona's ratio from its creation, the first
# reading was the host's and only a later one the persona's.
FRAME = ("<!doctype html><div id=b style='width:50px;height:10px;border-top:0.3px solid #000'></div>"
         "<script>const m = () => ({dpr: devicePixelRatio,"
         " res: matchMedia(`(resolution: ${devicePixelRatio}dppx)`).matches,"
         " border: document.getElementById('b').getBoundingClientRect().height - 10});"
         "parent.postMessage(m(), '*'); setTimeout(() => parent.postMessage(m(), '*'), 1500)</script>")
PAINT = "registerPaint('dpr', class { paint(ctx, size) { ctx.fillStyle = '#000'; ctx.fillRect(0, 0, devicePixelRatio * 20, size.height); } });"
PNG1 = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body, kind = {"/": (PAGE % PORT, "text/html"), "/frame": (FRAME, "text/html"),
                      "/paint.js": (PAINT, "text/javascript")}.get(self.path, (None, None))
        data = body.encode() if body else (PNG1 if self.path.startswith("/img") else b"")
        self.send_response(200 if data else 404)
        self.send_header("content-type", kind or "image/png")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), H)
PORT = srv.server_address[1]
threading.Thread(target=srv.serve_forever, daemon=True).start()


def png_rows(png):
    """RGBA/RGB 8-bit PNG to rows of (r,g,b)."""
    pos, chunks, w = 8, b"", None
    while pos < len(png):
        n, t = struct.unpack(">I4s", png[pos:pos + 8])
        d = png[pos + 8:pos + 8 + n]
        if t == b"IHDR":
            w, h, depth, ctype = struct.unpack(">IIBB", d[:10])
        elif t == b"IDAT":
            chunks += d
        pos += 12 + n
    bpp = 4 if ctype == 6 else 3
    raw, stride, rows, prev = zlib.decompress(chunks), w * bpp, [], bytearray(w * bpp)
    for y in range(h):
        f, line = raw[y * (stride + 1)], bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - bpp] if i >= bpp else 0
            b, c = prev[i], prev[i - bpp] if i >= bpp else 0
            if f == 1: line[i] = (line[i] + a) & 255
            elif f == 2: line[i] = (line[i] + b) & 255
            elif f == 3: line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append([tuple(line[x * bpp:x * bpp + 3]) for x in range(w)])
        prev = line
    return rows


def run(config):
    d = tempfile.mkdtemp(prefix="fury-dpr-")
    open(d + "/c.json", "w").write(json.dumps(config))
    fd = os.open(d + "/c.json", os.O_RDONLY); os.set_inheritable(fd, True)
    p = subprocess.Popen([CORE, f"--user-data-dir={d}", "--remote-debugging-port=0", "--no-first-run",
                          "--no-default-browser-check", "--use-mock-keychain", "--fury-fp-fd=3"],
                         preexec_fn=lambda: os.dup2(fd, 3), close_fds=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    os.close(fd)
    try:
        for _ in range(200):
            try: port = int(open(d + "/DevToolsActivePort").readline()); break
            except Exception: time.sleep(0.2)
        time.sleep(2); ws = browser(port)
        t = ws.call("Target.createTarget", {"url": f"http://127.0.0.1:{PORT}/"})["targetId"]
        s = ws.call("Target.attachToTarget", {"targetId": t, "flatten": True})["sessionId"]
        ws.call("Runtime.enable", session=s); time.sleep(3)
        out = ws.call("Runtime.evaluate", {"expression": "out", "returnByValue": True}, session=s)["result"]["value"]
        shot = ws.call("Page.captureScreenshot", {"format": "png", "clip": {"x": 10, "y": 10, "width": 200, "height": 20, "scale": 1}}, session=s)
        rows = png_rows(base64.b64decode(shot["data"]))
        mid = rows[len(rows) // 2]
        black = sum(1 for px in mid if sum(px) < 150)
        out["paintWorkletDpr"] = round(black / len(mid) * 10, 3)
        win = ws.call("Browser.getWindowForTarget", {"targetId": t})["bounds"]
        out["win"] = [win["width"], win["height"]]
        full = png_rows(base64.b64decode(ws.call("Page.captureScreenshot", {"format": "png"}, session=s)["data"]))
        red = [(x, y) for y, r in enumerate(full) for x, px in enumerate(r) if px[0] > 200 and px[1] < 60 and px[2] < 60]
        out["shot"] = [len(full[0]), len(full)]
        out["red"] = [min(x for x, _ in red), min(y for _, y in red), max(x for x, _ in red), max(y for _, y in red)] if red else None
        r = json.loads(ws.call("Runtime.evaluate", {"expression": "JSON.stringify(document.getElementById('btn').getBoundingClientRect())", "returnByValue": True}, session=s)["result"]["value"])
        x, y = r["x"] + r["width"] / 2, r["y"] + r["height"] / 2
        for kind in ("mousePressed", "mouseReleased"):
            ws.call("Input.dispatchMouseEvent", {"type": kind, "x": x, "y": y, "button": "left", "buttons": 1 if kind == "mousePressed" else 0, "clickCount": 1}, session=s)
        time.sleep(0.8)
        out["hits"] = ws.call("Runtime.evaluate", {"expression": "window.hits||0", "returnByValue": True}, session=s)["result"]["value"]
        # An odd window: 903 x 1.25 is not a whole number of device pixels.
        wid = ws.call("Browser.getWindowForTarget", {"targetId": t})["windowId"]
        ws.call("Browser.setWindowBounds", {"windowId": wid, "bounds": {"width": 903, "height": 653}})
        time.sleep(1.2)
        out["odd"] = ws.call("Runtime.evaluate", {"expression": "[innerWidth, innerHeight, outerWidth, outerHeight,"
                             " visualViewport.width, visualViewport.height]", "returnByValue": True}, session=s)["result"]["value"]
        return out
    finally:
        p.send_signal(signal.SIGHUP); p.wait(timeout=12); shutil.rmtree(d, ignore_errors=True)


results = []


def check(ok, text):
    results.append((ok, text))
    print(f"  {'OK  ' if ok else 'FAIL'} {text}", flush=True)


def near(a, b):
    return a is not None and abs(a - b) < 0.01


PHONE = {"schema_version": 1, "navigator": {"maxTouchPoints": 5},
         "screen": {"width": 384, "height": 832, "availWidth": 384, "availHeight": 832,
                    "devicePixelRatio": 2.8125, "chromeHeightDelta": 192, "colorDepth": 24},
         "mobile": {"enabled": True, "connectionType": "cellular", "touchSeed": 1}}

host = run({"schema_version": 1})
print(f"  host    {json.dumps(host)}")
check(host["dpr"] == 2, "this host is a 2x screen, so a persona at another ratio can be told from it")
# What a desktop persona must keep from the host run.
SAME = ("iw", "ih", "ow", "oh", "sw", "sh", "saw", "sah", "win", "shot", "hits")

for name, cfg, ratio, img in [
        ("phone", PHONE, 2.8125, "img3"),
        ("1x desktop", {"schema_version": 1, "screen": {"devicePixelRatio": 1.0}}, 1.0, "img1"),
        ("1.25x desktop", {"schema_version": 1, "screen": {"devicePixelRatio": 1.25}}, 1.25, "img2")]:
    o = run(cfg)
    print(f"\n--- {name} ---\n  {json.dumps(o)}")
    pixel = 1 / ratio if ratio > 1 else 1.0
    frames = o.get("frames") or []
    check(o["dpr"] == ratio and frames and all(f["dpr"] == ratio and f["res"] for f in frames),
          f"{name}: devicePixelRatio and the resolution media query are {ratio}, in the page "
          f"and in a cross-site frame")
    check(o["img"] == img, f"{name}: srcset loads {img} ({o['img']}; the host's img2)")
    check(near(o["border"], pixel),
          f"{name}: a 0.3px border rounds to one device pixel at {ratio}, {pixel:.3f} "
          f"(got {o['border']:.3f}; the host's 0.5)")
    # A frame whose layout has not run yet reads -10; that says nothing of
    # the ratio, so only real readings are judged, and there must be one.
    sized = [f["border"] for f in frames if f["border"] >= 0]
    check(len(frames) == 2 and sized and all(near(b, pixel) for b in sized),
          f"{name}: so does one in a cross-site frame, on load and later "
          f"({[round(f['border'], 3) for f in frames]})")
    check(o["dpcb"] == round(100 * ratio),
          f"{name}: devicePixelContentBoxSize of a 100px box is {round(100 * ratio)} "
          f"(got {o['dpcb']}; the host's 200)")
    check(abs(o["paintWorkletDpr"] - ratio) < 0.1,
          f"{name}: a paint worklet draws with devicePixelRatio {ratio} (measured {o['paintWorkletDpr']})")
    if cfg is PHONE:
        check(o["isExtended"] is False, "phone: screen.isExtended is false, a phone has one screen")
        continue
    iw, ih, ow, oh, vw, vh = o["odd"]
    h_iw, h_ih = host["odd"][:2]
    exact = ratio == 1.0 and (iw, ih) == (h_iw, h_ih)
    close = abs(iw - h_iw) <= 1 and abs(ih - h_ih) <= 1 and abs(vw - iw) < 1 and abs(vh - ih) < 1
    check(iw <= ow and ih <= oh and (exact if ratio == 1.0 else close),
          f"{name}: in a 903x653 window the page is {iw}x{ih} (visual viewport {vw:.2f}x{vh:.2f}), "
          f"the host's {h_iw}x{h_ih}{'' if ratio == 1.0 else ' to within the persona pixel'}, "
          f"never wider than the window's {ow}x{oh}")
    lost = {k: (host[k], o[k]) for k in SAME if host[k] != o[k]}
    check(not lost,
          f"{name}: window, inner and outer sizes, screen and its available area, the screenshot's "
          f"size and the click are the host run's (differ: {lost})")
    # The red square is fixed at the view's bottom-right: if the page were
    # drawn at the wrong size it would sit elsewhere in the real pixels.
    check(o["red"] and host["red"] and all(abs(a - b) <= 3 for a, b in zip(o["red"], host["red"])),
          f"{name}: the page fills the view on screen, its bottom-right corner where the host's is "
          f"({o['red']} vs {host['red']})")

srv.shutdown()
bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
