#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""How much slower is console.debug when something is driving the browser?

    ./cdp-timing.py <path-to-browser>

A page can time `console.debug(x)` and tell whether a debugger is attached. The
series has recorded this as "about thirteen times" since it was first seen; this
splits it into the two parts it is actually made of, which changes what can be
done about it.

Measured 04.08.2026 on Fury 150, microseconds per call, median of seven runs of
four hundred:

                          "hello"   50-key obj   800-key obj
    no debugger              3.0        2.8          2.8
    CDP attached, Runtime
      NOT enabled            2.8        2.8          2.8
    CDP attached, Runtime
      enabled                8.0       14.0         25.5

Three things follow.

Attaching is free. The cost arrives with `Runtime.enable`, which every driver
worth using calls — so "is CDP attached" is not what leaks; "is a driver
attached" is.

The cost has a FIXED part and a SIZE part. The size part is preview generation
and could be removed by a patch. The fixed part — five microseconds on a string,
2.7x — is the message reaching the frontend at all, and removing that means not
delivering Runtime.consoleAPICalled, which is the console.

So a patch can shrink this and cannot close it, and a patch that leaves the tell
is the failure mode core/patches/series exists to prevent. The control that
works is not opening CDP: it is off unless asked for, and a team server can
refuse it per role.

Real Chrome behaves the same way, measured the same way. This detects a driven
browser, not Fury.
"""

import asyncio, http.server, json, os, socketserver, subprocess, sys, threading, time
import urllib.request, websockets

if len(sys.argv) < 2:
    sys.exit(__doc__.strip().split("\n\n")[1])
CORE = sys.argv[1]
SP = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.environ.get("TMPDIR", "/tmp"), "cdp-timing")
os.makedirs(SP, exist_ok=True)
RESULTS = {}
PAGE = """<!doctype html><meta charset=utf-8><body><script>
(async () => {
  const mk = n => { const o={}; for (let i=0;i<n;i++) o['k'+i]={a:i,b:'x'.repeat(64),c:[1,2,3,4]}; return o; };
  const time = v => { const runs=[]; for (let r=0;r<7;r++){ const N=400,s=performance.now();
      for (let i=0;i<N;i++) console.debug(v); runs.push((performance.now()-s)/N); }
      runs.sort((a,b)=>a-b); return runs[3]; };
  const out = { small: time('hello'), obj50: time(mk(50)), obj800: time(mk(800)) };
  await fetch('/r', {method:'POST', body: JSON.stringify({perCall: out.obj800, all: out})});
})();
</script></body>"""

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        b = PAGE.encode(); self.send_response(200)
        self.send_header("content-type","text/html"); self.send_header("content-length",str(len(b)))
        self.end_headers(); self.wfile.write(b)
    def do_POST(self):
        n=int(self.headers.get("content-length",0))
        RESULTS[self.server.tag]=json.loads(self.rfile.read(n))
        self.send_response(204); self.end_headers()

def serve(tag):
    srv = socketserver.TCPServer(("127.0.0.1",0), H); srv.tag = tag
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, f"http://127.0.0.1:{srv.server_address[1]}/"

async def with_client(label, tag, port, enable_runtime):
    srv, url = serve(tag)
    p = subprocess.Popen([CORE,"--headless=new","--disable-gpu","--no-first-run",
        "--no-default-browser-check",f"--remote-debugging-port={port}",
        f"--user-data-dir={SP}/cdp3-{tag}","about:blank"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        ws_url=None
        for _ in range(80):
            try:
                ws_url=json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version",timeout=1))["webSocketDebuggerUrl"]; break
            except Exception: await asyncio.sleep(0.25)
        async with websockets.connect(ws_url, max_size=None) as ws:
            n=[0]
            async def call(m,pr=None,sid=None):
                n[0]+=1; i=n[0]
                msg={"id":i,"method":m,"params":pr or {}}
                if sid: msg["sessionId"]=sid
                await ws.send(json.dumps(msg))
                while True:
                    r=json.loads(await ws.recv())
                    if r.get("id")==i: return r.get("result",{})
            t=await call("Target.createTarget",{"url":"about:blank"})
            sid=(await call("Target.attachToTarget",{"targetId":t["targetId"],"flatten":True}))["sessionId"]
            if enable_runtime: await call("Runtime.enable",{},sid)
            await call("Page.navigate",{"url":url},sid)
            for _ in range(200):
                if tag in RESULTS: break
                await asyncio.sleep(0.1)
    finally:
        p.terminate(); srv.shutdown()
    r=RESULTS.get(tag)
    print((label+": "+", ".join(f"{k}={v*1000:.1f}us" for k,v in r["all"].items())) if r else label+": no answer")
    return r["perCall"] if r else None

def plain(label, tag):
    srv,url = serve(tag)
    p = subprocess.Popen([CORE,"--headless=new","--disable-gpu","--no-first-run",
        "--no-default-browser-check",f"--user-data-dir={SP}/cdp3-{tag}",url],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(200):
        if tag in RESULTS: break
        time.sleep(0.1)
    p.terminate(); srv.shutdown()
    r=RESULTS.get(tag)
    print((label+": "+", ".join(f"{k}={v*1000:.1f}us" for k,v in r["all"].items())) if r else label+": no answer")
    return r["perCall"] if r else None

async def main():
    base = plain("no debugger                 ", "none")
    a = await with_client("CDP attached, Runtime off   ", "noruntime", 9611, False)
    b = await with_client("CDP attached, Runtime on    ", "runtime",   9612, True)
    if base:
        for lbl,v in (("Runtime off", a), ("Runtime on ", b)):
            if v: print(f"  {lbl}: {v/base:.1f}x baseline")
asyncio.run(main())
