#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Does patch 0130 make a phone persona a phone in every context?

What it replaces was measured on 09.10.2026 (docs/18): DevTools' mobile
emulation over CDP made the main frame a phone and left a cross-site iframe
reporting the whole Mac. So the checks that matter most here are the ones in
the cross-site frame, which on a desktop with site isolation runs in another
renderer process, and in a Worker.

Each check is run twice, with a desktop config and with a phone one, and has to
come out different: a value that is "right" both times is the host agreeing by
accident, not the patch working.

Usage: core/verify/verify-0130.py <core binary>
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


# What a page can see of the phone-ness. Run in the top frame, in a dedicated
# Worker and in a cross-site iframe, which posts the same object back.
SURFACE = r"""
function surface() {
  const mm = q => matchMedia(q).matches;
  let notification;
  try { new Notification('x'); notification = 'constructed'; }
  catch (e) { notification = e.name; }
  return {
    touch: 'ontouchstart' in window,
    touchEvent: (() => { try { new TouchEvent('touchstart'); return true; } catch (e) { return false; } })(),
    coarse: mm('(pointer: coarse)'), fine: mm('(any-pointer: fine)'),
    hoverNone: mm('(hover: none)'), anyHover: mm('(any-hover: hover)'),
    maxTouchPoints: navigator.maxTouchPoints,
    connectionType: navigator.connection ? String(navigator.connection.type) : 'no connection',
    downlinkMax: navigator.connection && navigator.connection.downlinkMax === Infinity ? 'Infinity'
                 : navigator.connection ? navigator.connection.downlinkMax : null,
    pdf: navigator.pdfViewerEnabled, plugins: navigator.plugins.length,
    mimeTypes: navigator.mimeTypes.length,
    orientation: screen.orientation.type, angle: screen.orientation.angle,
    windowOrientation: 'orientation' in window ? window.orientation : 'absent',
    hid: 'hid' in navigator, serial: 'serial' in navigator,
    queryLocalFonts: 'queryLocalFonts' in window,
    pressure: 'PressureObserver' in window,
    docPip: 'documentPictureInPicture' in window,
    notification,
    innerWidth: innerWidth,
  };
}
"""

WORKER = r"""
self.onmessage = () => postMessage({
  connectionType: self.navigator.connection ? String(self.navigator.connection.type) : 'no connection',
  hid: 'hid' in self.navigator,
});
"""

TOP = """<!doctype html><title>top</title><script>%s
window.addEventListener('message', e => { window.frameSurface = e.data; });
</script>
<iframe src="http://localhost:%d/frame"></iframe>"""

FRAME = """<!doctype html><title>frame</title><script>%s
parent.postMessage(surface(), '*');
</script>"""

VIEWPORT = """<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1">
<title>vp</title>"""


class Pages(http.server.BaseHTTPRequestHandler):
    port = 0

    def do_GET(self):
        body, kind = {
            "/": (TOP % (SURFACE, Pages.port), "text/html"),
            "/frame": (FRAME % SURFACE, "text/html"),
            "/worker.js": (WORKER, "text/javascript"),
            "/vp": (VIEWPORT, "text/html"),
        }.get(self.path, ("", "text/plain"))
        data = body.encode()
        self.send_response(200 if body else 404)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Pages)
Pages.port = server.server_address[1]
threading.Thread(target=server.serve_forever, daemon=True).start()
TOP_URL = f"http://127.0.0.1:{Pages.port}/"


def launch(config):
    d = tempfile.mkdtemp(prefix="fury-0130-")
    args = [CORE, f"--user-data-dir={d}", "--remote-debugging-port=0",
            "--no-first-run", "--no-default-browser-check", "--use-mock-keychain",
            "--window-position=-4000,-4000", "--window-size=412,915"]
    cfg = os.path.join(d, "config.json")
    with open(cfg, "w") as f:
        json.dump(config, f)
    fd = os.open(cfg, os.O_RDONLY)
    os.set_inheritable(fd, True)
    args.append("--fury-fp-fd=3")
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
    ws = browser(port)
    return p, ws, d


def open_page(ws, url):
    t = ws.call("Target.createTarget", {"url": url})["targetId"]
    s = ws.call("Target.attachToTarget", {"targetId": t, "flatten": True})["sessionId"]
    ws.call("Runtime.enable", session=s)
    time.sleep(2.5)
    return s


def js(ws, s, expr, timeout=20000):
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


def measure(config):
    p, ws, d = launch(config)
    try:
        s = open_page(ws, TOP_URL)
        top = js(ws, s, "surface()")
        frame = js(ws, s, """new Promise(r => { const t0 = Date.now();
            (function wait() { if (window.frameSurface) r(window.frameSurface);
              else if (Date.now() - t0 > 8000) r({timedOut: true});
              else setTimeout(wait, 100); })(); })""")
        worker = js(ws, s, """new Promise(r => { const w = new Worker('/worker.js');
            w.onmessage = e => r(e.data); w.postMessage(1);
            setTimeout(() => r({timedOut: true}), 8000); })""")
        oopif = [t for t in ws.call("Target.getTargets")["targetInfos"]
                 if t.get("type") == "iframe" and "localhost" in t.get("url", "")]
        bare = top.get("innerWidth")
        s2 = open_page(ws, TOP_URL + "vp")
        meta = js(ws, s2, "innerWidth")
        return top, frame, worker, bool(oopif), bare, meta
    finally:
        stop(p, d)


DESKTOP = {"schema_version": 1}
PHONE = {"schema_version": 1,
         "navigator": {"maxTouchPoints": 5},
         "screen": {"width": 384, "height": 832},
         "mobile": {"enabled": True, "connectionType": "cellular"}}

print("\n--- desktop config: the host as it is ---")
d_top, d_frame, d_worker, d_oopif, d_bare, d_meta = measure(DESKTOP)
print(f"  top    {json.dumps(d_top)}")
print(f"  worker {json.dumps(d_worker)}")

print("\n--- phone config ---")
top, frame, worker, oopif, bare, meta = measure(PHONE)
print(f"  top    {json.dumps(top)}")
print(f"  frame  {json.dumps(frame)}")
print(f"  worker {json.dumps(worker)}")
print(f"  innerWidth without <meta viewport> {bare}, with width=device-width {meta}")

check(not d_top.get("touch") and d_top.get("fine"),
      "the desktop run has no touch events and a fine pointer, so the phone "
      "run below cannot pass by the host agreeing")
check(oopif, "the localhost iframe is its own target: a cross-site frame in "
      "another renderer, the context CDP emulation never reached")

for where, got in (("top frame", top), ("cross-site frame", frame)):
    check(got.get("touch") and got.get("touchEvent"),
          f"{where}: ontouchstart and TouchEvent exist")
    check(got.get("coarse") and got.get("hoverNone")
          and not got.get("fine") and not got.get("anyHover"),
          f"{where}: pointer coarse, no fine pointer, no hover")
    check(got.get("connectionType") == "cellular" and got.get("downlinkMax") == "Infinity",
          f"{where}: navigator.connection.type is the persona's cellular")
    check(got.get("pdf") is False and got.get("plugins") == 0 and got.get("mimeTypes") == 0,
          f"{where}: no PDF viewer, empty plugins and mimeTypes")
    check(got.get("orientation") == "portrait-primary" and got.get("angle") == 0
          and got.get("windowOrientation") == 0,
          f"{where}: portrait-primary at angle 0, window.orientation 0")
    check(not got.get("hid") and not got.get("queryLocalFonts")
          and not got.get("pressure") and not got.get("docPip"),
          f"{where}: no WebHID, Local Font Access, Compute Pressure or Document PiP")
    check(got.get("notification") == "TypeError",
          f"{where}: new Notification() throws, as on Android ({got.get('notification')})")
    check(got.get("serial") == d_top.get("serial"),
          f"{where}: navigator.serial left as Chrome 155 ships it (stable on Android too)")

check(worker.get("connectionType") == "cellular" and not worker.get("hid"),
      f"a Worker says cellular and has no WebHID: {json.dumps(worker)}")
check(d_worker.get("connectionType") != "cellular",
      f"and the desktop Worker did not: {json.dumps(d_worker)}")

check(bare == 980,
      f"a page with no <meta viewport> lays out at 980, as Chrome on Android "
      f"(got {bare}; desktop {d_bare})")
check(bare == 980 and isinstance(meta, int) and meta != 980 and meta == d_meta,
      f"and width=device-width brings it back to the window's width, so the "
      f"980 above is the viewport rule and not a wider window (got {meta}; "
      f"desktop {d_meta})")

server.shutdown()
bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
