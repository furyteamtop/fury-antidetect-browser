#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Does patch 0131 give a phone persona's pages the phone's screen, not the window's?

A Chrome window on macOS is never narrower than 500, so before this patch a
page using width=device-width saw a 500-wide viewport on a 384-wide phone
screen (docs/18). With the patch the main frame's widget runs Blink's screen
emulation from the config: innerWidth/innerHeight, devicePixelRatio and the
screen are the phone's whatever the window is, and a click still lands where
it is aimed.

Every claim is also run under a desktop config, which must answer with the
window, so the phone run cannot pass by the host agreeing.

Usage: core/verify/verify-0131.py <core binary>
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
<title>p</title><style>body{margin:0} #b{position:absolute;left:100px;top:200px;width:120px;height:60px}</style>
<button id="b" onclick="window.clicked=(window.clicked||0)+1">tap</button>"""
BARE = "<!doctype html><title>bare</title><p>no viewport meta"

SIZES = """JSON.stringify({iw: innerWidth, ih: innerHeight, ow: outerWidth, oh: outerHeight,
  sw: screen.width, sh: screen.height, dpr: devicePixelRatio,
  vw: visualViewport.width, orient: screen.orientation.type})"""


class Pages(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = {"/": PAGE, "/bare": BARE}.get(self.path, "")
        data = body.encode()
        self.send_response(200 if body else 404)
        self.send_header("content-type", "text/html")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Pages)
threading.Thread(target=server.serve_forever, daemon=True).start()
BASE = f"http://127.0.0.1:{server.server_address[1]}"


def launch(config):
    d = tempfile.mkdtemp(prefix="fury-0131-")
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


def open_page(ws, url):
    t = ws.call("Target.createTarget", {"url": url})["targetId"]
    s = ws.call("Target.attachToTarget", {"targetId": t, "flatten": True})["sessionId"]
    ws.call("Runtime.enable", session=s)
    time.sleep(2.5)
    return s


def js(ws, s, expr):
    r = ws.call("Runtime.evaluate", {"expression": expr, "awaitPromise": True,
                                     "returnByValue": True}, session=s)
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


def measure(config):
    p, ws, d = launch(config)
    try:
        s = open_page(ws, BASE + "/")
        meta = json.loads(js(ws, s, SIZES))
        # A real mouse click at the button's centre, in the coordinates the
        # page reports for it: what the browser sends is window DIPs, and the
        # emulation must map them back onto the same element.
        rect = json.loads(js(ws, s, "JSON.stringify(document.getElementById('b').getBoundingClientRect())"))
        x, y = rect["x"] + rect["width"] / 2, rect["y"] + rect["height"] / 2
        for kind in ("mousePressed", "mouseReleased"):
            ws.call("Input.dispatchMouseEvent", {"type": kind, "x": x, "y": y,
                                                 "button": "left", "clickCount": 1}, session=s)
        time.sleep(0.5)
        clicked = js(ws, s, "window.clicked || 0")
        s2 = open_page(ws, BASE + "/bare")
        bare = json.loads(js(ws, s2, SIZES))
        s3 = open_page(ws, "chrome://version")
        browser_page = json.loads(js(ws, s3, SIZES))
        return meta, clicked, bare, browser_page
    finally:
        stop(p, d)


DESKTOP = {"schema_version": 1}
PHONE = {"schema_version": 1,
         "navigator": {"maxTouchPoints": 5},
         "screen": {"width": 384, "height": 832, "availWidth": 384, "availHeight": 832,
                    "devicePixelRatio": 2.8125, "chromeHeightDelta": 192, "colorDepth": 24},
         "mobile": {"enabled": True, "connectionType": "cellular"}}

print("\n--- desktop config ---")
d_meta, d_clicked, d_bare, d_browser = measure(DESKTOP)
print(f"  meta {d_meta}\n  bare {d_bare}\n  chrome:// {d_browser}")
print("\n--- phone config ---")
meta, clicked, bare, browser_page = measure(PHONE)
print(f"  meta {meta}\n  bare {bare}\n  chrome:// {browser_page}\n  clicks {clicked}")

check(d_meta["iw"] >= 500 and d_meta["iw"] != 384,
      f"desktop: width=device-width is the window ({d_meta['iw']}), so 384 below is the patch")
check(meta["iw"] == 384 and meta["vw"] == 384,
      f"phone: width=device-width lays out at the phone's 384, not the window's (got {meta['iw']})")
check(meta["ih"] == 832 - 192,
      f"innerHeight is the screen less the browser's chrome, 640 (got {meta['ih']})")
check(meta["sw"] == 384 and meta["sh"] == 832 and abs(meta["dpr"] - 2.8125) < 1e-6,
      f"screen and devicePixelRatio are the phone's ({meta['sw']}x{meta['sh']} @ {meta['dpr']})")
check(meta["ow"] == 384 and meta["oh"] == 832,
      f"outerWidth/outerHeight come out as a phone's, the screen ({meta['ow']}x{meta['oh']})")
check(meta["orient"] == "portrait-primary", f"portrait ({meta['orient']})")
check(bare["iw"] == 980,
      f"a page with no <meta viewport> still lays out at Android's 980 (got {bare['iw']})")
check(bare["ow"] == 384 and bare["oh"] == 832,
      f"and its window is still the phone's screen, not inner + chrome in layout "
      f"pixels ({bare['ow']}x{bare['oh']})")
check(clicked == 1 and d_clicked == 1,
      f"a real mouse click lands on the button under emulation (phone {clicked}, desktop {d_clicked})")
check(browser_page["iw"] >= 500 and abs(browser_page["iw"] - browser_page["vw"]) < 1,
      f"chrome:// pages keep the desktop layout at the window's size, not "
      f"Android's 980 zoomed out ({browser_page['iw']} wide, visual {browser_page['vw']})")

server.shutdown()
bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
