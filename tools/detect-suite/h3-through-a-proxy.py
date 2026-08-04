#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Does a browser use HTTP/3 when it is behind a proxy?

    ./h3-through-a-proxy.py --proxy socks5://127.0.0.1:1080 [--url URL]

Answers the question the roadmap had answered the wrong way round for months:
Fury never negotiates HTTP/3, and that was written down as a difference from
Chrome that a server advertising alt-svc could see.

It is not. Chromium does not carry QUIC through a proxy at all — QUIC is UDP and
--proxy-server speaks TCP CONNECT — so real Chrome behind a proxy is h2 too, and
a Fury profile is always behind one. Measured 04.08.2026, real Chrome 150:

    cloudflare-quic.com    h3 x1     ->  h2 x1
    google.com             h3 x28    ->  h2 x27
    blog.cloudflare.com    h3 x36    ->  h2 x36

Kept as a script rather than a note because the next Chromium may change it, and
because the same question will be asked about Fury's own core. Reads
`nextHopProtocol` off every resource the page loaded, which is what a server
would see rather than what a flag claims.
"""

import asyncio, json, subprocess, sys, time, urllib.request
import websockets

import argparse, os

DEFAULT_CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

async def measure(label, extra, port, profile, args):
    argv = [args.browser, "--headless=new", "--disable-gpu", f"--remote-debugging-port={port}",
            f"--user-data-dir={profile}", "--no-first-run", "--no-default-browser-check",
            "--enable-quic"] + extra
    p = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        ws_url = None
        for _ in range(60):
            try:
                v = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version", timeout=1))
                ws_url = v["webSocketDebuggerUrl"]; break
            except Exception:
                await asyncio.sleep(0.25)
        if not ws_url:
            print(f"{label}: браузер не поднялся"); return
        async with websockets.connect(ws_url, max_size=None) as ws:
            await ws.send(json.dumps({"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}))
            tid=None
            while True:
                m=json.loads(await ws.recv())
                if m.get("id")==1: tid=m["result"]["targetId"]; break
            await ws.send(json.dumps({"id":2,"method":"Target.attachToTarget","params":{"targetId":tid,"flatten":True}}))
            sid=None
            while True:
                m=json.loads(await ws.recv())
                if m.get("id")==2: sid=m["result"]["sessionId"]; break
            async def send(i, method, params=None):
                await ws.send(json.dumps({"id":i,"method":method,"params":params or {},"sessionId":sid}))
            await send(3,"Page.enable"); await send(4,"Page.navigate",{"url":args.url})
            # дать загрузиться
            t0=time.time()
            while time.time()-t0 < 25:
                try:
                    m=json.loads(await asyncio.wait_for(ws.recv(), timeout=2))
                except asyncio.TimeoutError:
                    break
            await send(9,"Runtime.evaluate",{"expression":
                "JSON.stringify(performance.getEntriesByType('resource').concat(performance.getEntriesByType('navigation')).map(e=>e.nextHopProtocol).filter(Boolean))",
                "returnByValue":True})
            while True:
                m=json.loads(await asyncio.wait_for(ws.recv(), timeout=20))
                if m.get("id")==9:
                    val=m["result"]["result"].get("value")
                    protos=json.loads(val) if val else []
                    from collections import Counter
                    print(f"{label}: {dict(Counter(protos))}")
                    return
    finally:
        p.terminate()

async def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--proxy", required=True, help="e.g. socks5://127.0.0.1:1080")
    ap.add_argument("--url", default="https://blog.cloudflare.com/",
                    help="a site that offers HTTP/3")
    # Defaults to the installed Chrome and NOT to FURY_CORE, deliberately: the
    # point of this script is the comparison, and quietly measuring Fury twice
    # would produce two identical numbers and no information. Point --browser at
    # a core when you want Fury's side of it.
    ap.add_argument("--browser", default=DEFAULT_CHROME,
                    help="which browser to drive (default: the installed Chrome)")
    args = ap.parse_args()

    tmp = os.path.join(os.environ.get("TMPDIR", "/tmp"), "h3-probe")
    # Both, always. A single number is not a measurement: "no h3 through the
    # proxy" only means something beside "h3 without one" on the same machine,
    # the same browser and the same page.
    await measure("direct       ", [], 9403, f"{tmp}/a", args)
    await measure(f"via {args.proxy}", [f"--proxy-server={args.proxy}"], 9404, f"{tmp}/b", args)

asyncio.run(main())
