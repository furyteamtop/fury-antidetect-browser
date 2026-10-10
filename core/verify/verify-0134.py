#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Does patch 0134 put a phone persona's page in a phone-wide column?

The column's position cannot be read from inside it: CDP and the page both
count from the view's own corner. Its width can, on a page 0131 does not
emulate: chrome://version reports the real width of the view it is drawn in.
Under a phone config that must be the phone's 384 in a window that macOS will
not make narrower than 500; under a desktop config, the window's width. The
column is centred by the layout code that narrows it ((window - phone) / 2 on
each side), so the width is the measurable half of the claim.

A click on a page under emulation must still land on its element, which is
the half that would break if the view moved and input did not follow.

Usage: core/verify/verify-0134.py <core binary>
See core/verify/README.md.
"""

import http.server
import json
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


def check(ok, text):
    results.append((ok, text))
    print(f"  {'OK  ' if ok else 'FAIL'} {text}", flush=True)


PAGE = """<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1">
<title>c</title><style>body{margin:0} #b{position:absolute;left:150px;top:120px;width:80px;height:50px}</style>
<button id="b" onclick="window.hits=(window.hits||0)+1">x</button>"""


class Pages(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        data = PAGE.encode() if self.path == "/" else b""
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


def measure(config):
    d = tempfile.mkdtemp(prefix="fury-0134-")
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

        def page(url):
            t = ws.call("Target.createTarget", {"url": url})["targetId"]
            s = ws.call("Target.attachToTarget", {"targetId": t, "flatten": True})["sessionId"]
            ws.call("Runtime.enable", session=s)
            time.sleep(2)
            return t, s

        def js(s, expr):
            return ws.call("Runtime.evaluate", {"expression": expr, "returnByValue": True},
                           session=s)["result"].get("value")

        t, s = page("chrome://version")
        win = ws.call("Browser.getWindowForTarget", {"targetId": t})["bounds"]
        view = js(s, "innerWidth")
        _, s2 = page(URL)
        r = json.loads(js(s2, "JSON.stringify(document.getElementById('b').getBoundingClientRect())"))
        x, y = r["x"] + r["width"] / 2, r["y"] + r["height"] / 2
        for kind in ("mousePressed", "mouseReleased"):
            ws.call("Input.dispatchMouseEvent", {"type": kind, "x": x, "y": y, "button": "left",
                                                 "buttons": 1 if kind == "mousePressed" else 0,
                                                 "clickCount": 1}, session=s2)
        time.sleep(0.8)
        hits = js(s2, "window.hits || 0")
        return win["width"], view, hits
    finally:
        p.send_signal(signal.SIGHUP)
        try:
            p.wait(timeout=12)
        except subprocess.TimeoutExpired:
            p.kill()
        shutil.rmtree(d, ignore_errors=True)


DESKTOP = {"schema_version": 1}
PHONE = {"schema_version": 1,
         "navigator": {"maxTouchPoints": 5},
         "screen": {"width": 384, "height": 832, "availWidth": 384, "availHeight": 832,
                    "devicePixelRatio": 2.8125, "chromeHeightDelta": 192, "colorDepth": 24},
         "mobile": {"enabled": True, "connectionType": "cellular"}}

d_win, d_view, d_hits = measure(DESKTOP)
print(f"  desktop: window {d_win}, view {d_view}, clicks {d_hits}")
win, view, hits = measure(PHONE)
print(f"  phone:   window {win}, view {view}, clicks {hits}; column at {(win - view) / 2:.0f} px from each side")

check(d_view == d_win, f"desktop: the view fills the window ({d_view} of {d_win})")
check(view == 384 and win > 384,
      f"phone: the view is a 384-wide column in a {win}-wide window, not the whole width")
check(hits == 1 and d_hits == 1,
      f"a click on the moved view still lands on its element (phone {hits}, desktop {d_hits})")

server.shutdown()
bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
