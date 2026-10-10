#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Does patch 0133 open a phone persona's browser window at the phone's size?

0131 gives the page the phone's screen whatever the window is and draws it at
the window's top-left, so without this the phone sat in the corner of a
desktop-sized window. The window must come out phone-sized however it was
asked for: a --window-size on the command line, and a session restored at the
size a person left it. A desktop config must keep the size it asked for, so
the phone run cannot pass by the window happening to be small.

Usage: core/verify/verify-0133.py <core binary>
See core/verify/README.md.
"""

import json
import os
import shutil
import signal
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


def launch(config, d, extra=()):
    # No --window-size: Fury's launcher never passes one, and Chrome applies
    # that switch after every other decision about the window
    # (browser_window_state.cc, UpdateWindowBoundsAndShowStateFromCommandLine).
    args = [CORE, f"--user-data-dir={d}", "--remote-debugging-port=0", "--no-first-run",
            "--no-default-browser-check", "--use-mock-keychain", "--fury-fp-fd=3", *extra]
    cfg = os.path.join(d, "config.json")
    with open(cfg, "w") as f:
        json.dump(config, f)
    fd = os.open(cfg, os.O_RDONLY)
    os.set_inheritable(fd, True)
    pf = os.path.join(d, "DevToolsActivePort")
    if os.path.exists(pf):
        os.remove(pf)
    p = subprocess.Popen(args, preexec_fn=lambda: os.dup2(fd, 3), close_fds=False,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    os.close(fd)
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
    time.sleep(2)
    return p, browser(port)


def window(ws):
    pages = [t for t in ws.call("Target.getTargets")["targetInfos"] if t["type"] == "page"]
    w = ws.call("Browser.getWindowForTarget", {"targetId": pages[0]["targetId"]})
    return w["windowId"], w["bounds"]


def close(p):
    # SIGHUP, not SIGTERM: on macOS SIGTERM is session-ending and skips
    # writing the session that the second launch restores.
    p.send_signal(signal.SIGHUP)
    try:
        p.wait(timeout=15)
    except subprocess.TimeoutExpired:
        p.kill()


DESKTOP = {"schema_version": 1}
PHONE = {"schema_version": 1,
         "screen": {"width": 384, "height": 832, "availWidth": 384, "availHeight": 832,
                    "devicePixelRatio": 2.8125, "chromeHeightDelta": 192, "colorDepth": 24},
         "mobile": {"enabled": True, "connectionType": "cellular"}}
WANT_H = (832 - 192) + 87 + 24

d = tempfile.mkdtemp(prefix="fury-0133-")
try:
    print("\n--- desktop config ---")
    p, ws = launch(DESKTOP, d)
    _, desk = window(ws)
    print(f"  window {desk}")
    close(p)
    check(desk["width"] > 700 and desk["height"] != WANT_H,
          f"desktop: the window is Chrome's own default, not a phone ({desk['width']}x{desk['height']})")
finally:
    shutil.rmtree(d, ignore_errors=True)

d = tempfile.mkdtemp(prefix="fury-0133-")
try:
    print("\n--- phone config ---")
    p, ws = launch(PHONE, d)
    wid, first = window(ws)
    print(f"  first launch {first}")
    check(first["height"] == WANT_H,
          f"phone: the window is the page's height plus the toolbar and margin, {WANT_H} "
          f"(got {first['height']})")
    check(first["width"] < desk["width"] and first["width"] >= 384 + 48,
          f"and narrow, the phone plus a margin or the platform's minimum (got {first['width']})")

    # A person stretches it and quits; the session comes back.
    ws.call("Browser.setWindowBounds", {"windowId": wid, "bounds": {"width": 1000, "height": 900}})
    time.sleep(1)
    _, stretched = window(ws)
    print(f"  stretched to {stretched}")
    close(p)
    p, ws = launch(PHONE, d, extra=("--restore-last-session",))
    _, again = window(ws)
    print(f"  restored {again}")
    close(p)
    check(stretched["width"] >= 1000,
          f"the window could be stretched (to {stretched['width']}x{stretched['height']}), so "
          f"the next check is about the restore")
    check(again["height"] == WANT_H and again["width"] == first["width"],
          f"a restored session opens phone-sized again ({again['width']}x{again['height']})")
finally:
    shutil.rmtree(d, ignore_errors=True)

bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
