#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""The User-Agent, and the four other places that have to say the same thing.

The UA string is the one field everybody spoofs and the one that proves least,
because Chrome now answers the same question four more ways and a site can ask
all five:

  * the `User-Agent` HTTP header
  * `navigator.userAgent`
  * `navigator.userAgentData.brands` and `.platform` — the low-entropy hints,
    sent on every request as `Sec-CH-UA`
  * `navigator.userAgentData.getHighEntropyValues()` — architecture, bitness,
    model, platformVersion, fullVersionList
  * the `Sec-CH-UA-*` request headers themselves

A profile that changes the string and leaves the hints is announcing Windows in
one field and macOS in another, which is a stronger signal than not spoofing at
all. So the header is read off a local server rather than asserted about, and
every claim is a claim about AGREEMENT.

One thing is deliberately NOT checked: that the brand list stops naming Chrome.
It must keep naming Chrome. A browser whose Sec-CH-UA says something else is
one of a handful in the world; see the note in the series file.

Usage: core/verify/verify-0011.py <core binary>
"""

import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# A Windows 11 machine, chosen because this one is not.
UA = ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36")
# The brand list a persona actually produces (shared-rs/src/persona.rs), in the
# "Name/version" form the patch parses. Left unset, the build answers
# "Chromium" alone — which is not what any profile ships and would make this
# script test a state nobody runs.
BRANDS = ["Not;A=Brand/8", "Chromium/150", "Google Chrome/150"]
FULL_VERSION_LIST = ["Not;A=Brand/8.0.0.0", "Chromium/150.0.7871.187",
                     "Google Chrome/150.0.7871.187"]

HINTS = {
    "platform": "Windows",
    "platformVersion": "15.0.0",
    "architecture": "x86",
    "bitness": "64",
    "model": "",
    "mobile": False,
    "wow64": False,
    "fullVersion": "150.0.7871.187",
}

READ = """
(async () => {
  const d = navigator.userAgentData;
  const high = await d.getHighEntropyValues([
    'architecture', 'bitness', 'model', 'platformVersion',
    'fullVersionList', 'wow64', 'formFactors']);
  return JSON.stringify({
    ua: navigator.userAgent,
    appVersion: navigator.appVersion,
    platform: d.platform,
    mobile: d.mobile,
    brands: d.brands,
    high,
  });
})()
"""


def serve_and_record():
    """A page served from here, so the request headers can be read."""
    seen = {}

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            seen.update({k.lower(): v for k, v in self.headers.items()})
            body = b"<!doctype html><title>hints</title><body>ok"
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            # Ask for the high-entropy hints, or Chrome sends only the low ones.
            self.send_header("Accept-CH", "Sec-CH-UA-Platform-Version, "
                                          "Sec-CH-UA-Arch, Sec-CH-UA-Bitness, "
                                          "Sec-CH-UA-Model, Sec-CH-UA-Full-Version-List")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, seen


def main():
    claims = Claims("0011 — UA string and the client hints beside it", CORE)

    config = {
        "navigator": {"userAgent": UA},
        "clientHints": dict(HINTS, brands=BRANDS,
                            fullVersionList=FULL_VERSION_LIST),
    }

    server, seen = serve_and_record()
    url = f"http://127.0.0.1:{server.server_port}/"
    with launch(CORE, config, page=url) as s:
        # A second navigation, so the Accept-CH from the first has taken effect.
        s.js(f"location.href = {json.dumps(url)}")
        import time
        time.sleep(2)
        got = json.loads(s.js(READ))
        print(f"  ua: {got['ua']}")
        print(f"  brands: {got['brands']}")
        print(f"  high: {got['high']}")
        print(f"  headers: { {k: v for k, v in seen.items() if k.startswith('sec-ch') or k == 'user-agent'} }")

        claims.check(got["ua"] == UA, f"navigator.userAgent is the persona's")
        claims.check(seen.get("user-agent") == UA,
                     f"and so is the User-Agent header — one source, not two "
                     f"(got {seen.get('user-agent')!r})")
        claims.check(got["appVersion"] == UA.removeprefix("Mozilla/"),
                     f"navigator.appVersion is the same string without the "
                     f"Mozilla/ prefix, as it is in every Chrome "
                     f"(got {got['appVersion']!r})")

        claims.check(got["platform"] == HINTS["platform"],
                     f"userAgentData.platform says {HINTS['platform']} — a UA "
                     f"claiming Windows beside a hint saying macOS is louder "
                     f"than no spoof at all (got {got['platform']!r})")
        claims.check(got["mobile"] is False,
                     f"and userAgentData.mobile is false (got {got['mobile']!r})")

        for key in ("architecture", "bitness", "model", "platformVersion"):
            claims.check(got["high"][key] == HINTS[key],
                         f"getHighEntropyValues {key!r} is {HINTS[key]!r} "
                         f"(got {got['high'][key]!r})")
        claims.check(got["high"]["wow64"] is False,
                     f"and wow64 is false, as it is for a native x64 build "
                     f"(got {got['high']['wow64']!r})")

        # The headers, which is where a hint-only spoof shows.
        claims.check(seen.get("sec-ch-ua-platform") == f'"{HINTS["platform"]}"',
                     f"the Sec-CH-UA-Platform header agrees with the JS "
                     f"(got {seen.get('sec-ch-ua-platform')!r})")
        claims.check(seen.get("sec-ch-ua-arch") == f'"{HINTS["architecture"]}"'
                     and seen.get("sec-ch-ua-bitness") == f'"{HINTS["bitness"]}"',
                     f"and so do Sec-CH-UA-Arch and -Bitness "
                     f"({seen.get('sec-ch-ua-arch')!r}, "
                     f"{seen.get('sec-ch-ua-bitness')!r})")

        # The claim that is a claim about NOT changing something.
        brand_names = [b["brand"] for b in got["brands"]]
        claims.check("Google Chrome" in brand_names and "Chromium" in brand_names,
                     f"the brand list still names Google Chrome AND Chromium, as "
                     f"a real Chrome does — a browser whose Sec-CH-UA says "
                     f"anything else is one of a handful in the world "
                     f"(got {brand_names})")
        claims.check("Google Chrome" in seen.get("sec-ch-ua", ""),
                     f"and the Sec-CH-UA header carries the same list "
                     f"(got {seen.get('sec-ch-ua')!r})")
        full = {b["brand"]: b["version"] for b in got["high"]["fullVersionList"]}
        claims.check(full.get("Google Chrome") == HINTS["fullVersion"],
                     f"and fullVersionList gives Google Chrome the persona's "
                     f"full version (got {full})")

    server.shutdown()

    server2, seen2 = serve_and_record()
    with launch(CORE, None, page=f"http://127.0.0.1:{server2.server_port}/") as s:
        bare = json.loads(s.js(READ))
        print(f"  unconfigured: {bare['platform']!r} {bare['ua'][:60]}...")
        claims.control(bare["ua"] != UA and bare["platform"] != HINTS["platform"],
                       f"an unconfigured build reports this machine in both the "
                       f"string and the hints (got {bare['platform']!r})")
    server2.shutdown()

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
