#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Does the host screen's device pixel ratio show through a persona's?

window.devicePixelRatio is only the number a page asks for first. The ratio
also decides where a thin border rounds to, devicePixelContentBoxSize, which
srcset candidate loads, the resolution media queries, and what a CSS paint
worklet reads. Measured on 10.10.2026 with a 2.8125 phone persona on a 2x
Mac: the border, devicePixelContentBoxSize and the paint worklet all said 2
(fixed in 0131, the compositor at the phone's ratio, and 0020, the worklet).

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
out.hasScreenDetails = 'getScreenDetails' in window;
addEventListener('message', e => out.frame = e.data);
setTimeout(() => { out.img = document.getElementById('i').currentSrc.split('/').pop(); }, 800);
</script>"""
FRAME = "<!doctype html><script>parent.postMessage({dpr: devicePixelRatio, res: matchMedia('(resolution: 2.8125dppx)').matches}, '*')</script>"
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
        return out
    finally:
        p.send_signal(signal.SIGHUP); p.wait(timeout=12); shutil.rmtree(d, ignore_errors=True)


results = []


def check(ok, text):
    results.append((ok, text))
    print(f"  {'OK  ' if ok else 'FAIL'} {text}", flush=True)


PHONE = {"schema_version": 1, "navigator": {"maxTouchPoints": 5},
         "screen": {"width": 384, "height": 832, "availWidth": 384, "availHeight": 832,
                    "devicePixelRatio": 2.8125, "chromeHeightDelta": 192, "colorDepth": 24},
         "mobile": {"enabled": True, "connectionType": "cellular", "touchSeed": 1}}
ONE_X = {"schema_version": 1, "screen": {"devicePixelRatio": 1.0}}

host = run({"schema_version": 1})
phone = run(PHONE)
one = run(ONE_X)
print(f"  host    {json.dumps(host)}\n  phone   {json.dumps(phone)}\n  1x desk {json.dumps(one)}")
check(host["dpr"] == 2, "this host is a 2x screen, so a phone at 2.8125 can be told from it")
check(phone["dpr"] == 2.8125 and phone["frame"]["dpr"] == 2.8125 and phone["frame"]["res"],
      "phone: devicePixelRatio is 2.8125 in the page and in a cross-site frame")
check(phone["res2812"] and phone["webkit"] and not phone["res2"],
      "phone: the resolution media queries match 2.8125, not 2")
check(phone["img"] == "img3", f"phone: srcset loads the 3x candidate ({phone['img']})")
check(abs(phone["border"] - 1 / 2.8125) < 0.01,
      f"phone: a 0.3px border rounds to one of the phone's device pixels, 0.356 "
      f"(got {phone['border']:.3f}; the host's would be 0.5)")
check(phone["dpcb"] == 281,
      f"phone: devicePixelContentBoxSize of a 100px box is 281 (got {phone['dpcb']}; the host's 200)")
check(abs(phone["paintWorkletDpr"] - 2.8125) < 0.1,
      f"phone: a paint worklet draws with devicePixelRatio 2.8125 (measured {phone['paintWorkletDpr']})")
check(phone["isExtended"] is False, "phone: screen.isExtended is false, a phone has one screen")
check(abs(one["paintWorkletDpr"] - 1.0) < 0.1,
      f"a desktop persona at 1x on this 2x host: the paint worklet says 1 too "
      f"(measured {one['paintWorkletDpr']}; 0020)")
# Known and not fixed: on a desktop persona whose ratio differs from the host
# the border and devicePixelContentBoxSize still follow the host. Printed so
# the number is seen on every run.
print(f"  NOTE desktop persona at 1x on a 2x host: border {one['border']}, "
      f"devicePixelContentBox {one['dpcb']} (the host's, not fixed)")
srv.shutdown()
bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
