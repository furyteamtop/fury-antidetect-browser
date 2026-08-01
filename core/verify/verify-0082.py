#!/usr/bin/env python3
"""Does patch 0082 do what its commit message says?

Runs the built core directly — no agent, no proxy, no relay — so that what
passes or fails here is the patch and nothing around it. Two runs: one with a
position in the config and one without, because "returns the configured
position" and "leaves the machine alone when there is nothing to configure" are
different claims and only the pair of them is the design.

Usage: core/verify/verify-0082.py <core binary>
See core/verify/README.md.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cdp import WS, browser  # noqa: E402

CORE = sys.argv[1]
# Somewhere unambiguous, and not where this machine is: if the browser reports
# these numbers they came from the config and cannot have come from CoreLocation.
LAT, LNG = 52.520008, 13.404954  # Berlin, Mitte
ACCURACY = 20000.0
ORIGIN = "https://example.com"

results = []


def check(ok, text):
    results.append((ok, text))
    print(f"  {'OK  ' if ok else 'FAIL'} {text}", flush=True)


def launch(config):
    """Start the core with `config` on fd 3 and return (proc, ws, datadir)."""
    datadir = tempfile.mkdtemp(prefix="fury-0082-")
    cfg = os.path.join(datadir, "config.json")
    with open(cfg, "w") as f:
        json.dump(config, f)
    fd = os.open(cfg, os.O_RDONLY)
    os.set_inheritable(fd, True)
    # Move it to 3 in the child. posix_spawn keeps the number we ask for.
    proc = subprocess.Popen(
        [
            CORE,
            f"--user-data-dir={datadir}",
            "--fury-fp-fd=3",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-field-trial-config",
            "--disable-background-networking",
            # Headed, off the visible desktop. Not a detail: --headless=new
            # denies notifications by policy, so the first run of this script
            # reported permissions.query({name:'notifications'}) as "denied" and
            # blamed patch 0090 for it. Fury never runs headless, and a check
            # that runs the browser in a configuration the product never uses
            # answers a question nobody asked.
            "--window-position=-4000,-4000",
            "--window-size=600,400",
        ],
        preexec_fn=(lambda: os.dup2(fd, 3)),
        close_fds=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    os.close(fd)

    portfile = os.path.join(datadir, "DevToolsActivePort")
    for _ in range(300):
        if os.path.exists(portfile):
            with open(portfile) as f:
                port = int(f.readline().strip())
            if port:
                break
        if proc.poll() is not None:
            raise RuntimeError(f"core exited {proc.returncode}: {proc.stderr.read()[-800:]}")
        time.sleep(0.2)
    else:
        raise RuntimeError("no DevToolsActivePort")

    time.sleep(0.5)
    return proc, browser(port), datadir


def page(ws, url):
    target = ws.call("Target.createTarget", {"url": "about:blank"})["targetId"]
    session = ws.call("Target.attachToTarget", {"targetId": target, "flatten": True})["sessionId"]
    ws.call("Page.enable", session=session)
    ws.call("Runtime.enable", session=session)
    ws.call("Page.navigate", {"url": url}, session=session)
    time.sleep(3.0)
    return session


def js(ws, session, expr):
    r = ws.call(
        "Runtime.evaluate",
        {"expression": expr, "awaitPromise": True, "returnByValue": True, "timeout": 20000},
        session=session,
    )
    if "exceptionDetails" in r:
        return {"threw": str(r["exceptionDetails"].get("text"))}
    return r["result"].get("value")


GET = """
new Promise(r => navigator.geolocation.getCurrentPosition(
  p => r({ok: true, lat: p.coords.latitude, lng: p.coords.longitude,
           acc: p.coords.accuracy, alt: p.coords.altitude,
           heading: p.coords.heading, speed: p.coords.speed, ts: p.timestamp}),
  e => r({ok: false, code: e.code, message: e.message}),
  {timeout: 15000}))
"""

WATCH = """
new Promise(r => {
  const seen = [];
  const id = navigator.geolocation.watchPosition(
    p => { seen.push({lat: p.coords.latitude, ts: p.timestamp});
           if (seen.length >= 2) { navigator.geolocation.clearWatch(id); r(seen); } },
    e => r({error: e.code}));
  setTimeout(() => { navigator.geolocation.clearWatch(id); r(seen); }, 12000);
})
"""


def with_position():
    print("\n--- a profile that knows where its exit is ---")
    proc, ws, datadir = launch({
        "schema_version": 1,
        "geolocation": {"latitude": LAT, "longitude": LNG, "accuracy": ACCURACY},
        "permissions": {"geolocation": "prompt", "notifications": "prompt"},
    })
    try:
        s = page(ws, ORIGIN)

        # Asked BEFORE the grant below, and that ordering is the whole point.
        # Browser.grantPermissions does not add to the origin's permissions, it
        # REPLACES them — everything not in the list is set to denied. Asking
        # afterwards reported notifications as "denied" and I spent a rebuild
        # blaming patch 0090 for it.
        notif = js(ws, s, "navigator.permissions.query({name:'notifications'}).then(p=>p.state)")
        check(notif == "prompt", f"an untouched permission reports the persona's {notif!r}")

        # The grant a user would give by clicking Allow. Everything downstream
        # of it — the provider, the caching, the handshake — is stock Chromium.
        ws.call("Browser.grantPermissions", {"origin": ORIGIN, "permissions": ["geolocation"]})

        pos = js(ws, s, GET)
        if not isinstance(pos, dict) or not pos.get("ok"):
            check(False, f"getCurrentPosition did not answer: {pos}")
        else:
            check(abs(pos["lat"] - LAT) < 1e-6 and abs(pos["lng"] - LNG) < 1e-6,
                  f"position is the configured one: {pos['lat']}, {pos['lng']}")
            check(pos["acc"] == ACCURACY, f"accuracy is the configured {pos['acc']} m")
            check(pos["alt"] is None and pos["heading"] is None and pos["speed"] is None,
                  "altitude, heading and speed are null — a desktop has no such sensors")
            drift = abs(pos["ts"] / 1000.0 - time.time())
            check(drift < 120, f"timestamp is now, not the moment the config was written ({drift:.0f}s off)")

        # Patch 0090's guard. Before it, this said "prompt" while the position
        # above was sitting in the same page — a browser contradicting itself.
        perm = js(ws, s, "navigator.permissions.query({name:'geolocation'}).then(p=>p.state)")
        check(perm == "granted", f"permissions.query agrees with the grant: {perm!r}")

        # The failure mode that ruled out Emulation.setGeolocationOverride:
        # one position per binding and then silence, which starves a watch.
        seen = js(ws, s, WATCH)
        check(isinstance(seen, list) and len(seen) >= 1,
              f"watchPosition is fed rather than starved: {len(seen) if isinstance(seen, list) else seen} fix(es)")

        # A one-shot while a watch is already outstanding. Blink keeps exactly
        # one QueryNextPosition in flight per frame, so a provider that speaks
        # once per start cycle can leave this one waiting forever — the same
        # starvation that ruled out setGeolocationOverride, arrived at from the
        # other direction.
        both = js(ws, s, """
        new Promise(r => {
          const w = navigator.geolocation.watchPosition(()=>{}, ()=>{});
          setTimeout(() => navigator.geolocation.getCurrentPosition(
            p => { navigator.geolocation.clearWatch(w); r({ok:true, lat:p.coords.latitude}); },
            e => { navigator.geolocation.clearWatch(w); r({ok:false, code:e.code}); },
            {timeout: 10000}), 1500);
        })""")
        check(isinstance(both, dict) and both.get("ok"),
              f"getCurrentPosition still answers while a watch is open: {both}")

        # The gate is real: denied means denied, not "denied unless you ask the
        # provider directly".
        ws.call("Browser.setPermission",
                {"origin": ORIGIN,
                 "permission": {"name": "geolocation"},
                 "setting": "denied"})
        denied = js(ws, s, GET)
        check(isinstance(denied, dict) and denied.get("ok") is False and denied.get("code") == 1,
              f"a denied origin gets PERMISSION_DENIED and no coordinates: {denied}")
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            proc.kill()
        shutil.rmtree(datadir, ignore_errors=True)


def without_position():
    print("\n--- a profile whose exit could not be placed ---")
    proc, ws, datadir = launch({
        "schema_version": 1,
        "permissions": {"geolocation": "prompt", "notifications": "prompt"},
    })
    try:
        ws.call("Browser.grantPermissions", {"origin": ORIGIN, "permissions": ["geolocation"]})
        s = page(ws, ORIGIN)
        pos = js(ws, s, GET)
        invented = (isinstance(pos, dict) and pos.get("ok")
                    and abs(pos.get("lat", 0) - LAT) < 1e-6)
        check(not invented,
              f"no position in the config means no Fury provider — the machine answers: {pos}")
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            proc.kill()
        shutil.rmtree(datadir, ignore_errors=True)


with_position()
without_position()

bad = [t for ok, t in results if not ok]
print(f"\n{len(results) - len(bad)}/{len(results)} checks passed")
sys.exit(1 if bad else 0)
