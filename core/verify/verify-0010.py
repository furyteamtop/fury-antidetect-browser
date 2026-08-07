#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""platform, deviceMemory, maxTouchPoints, languages — and one header.

Four properties every fingerprinting script reads in its first ten lines, and
one thing that is not a property at all: `Accept-Language`. That header is built
in //net from a profile preference rather than from `navigator.languages`, so a
patch that sets the JavaScript value and stops leaves the browser announcing one
language over the wire and another to the page. The two disagreeing is a tell
that no amount of navigator spoofing hides, and it is checked here rather than
assumed.

`deviceMemory` is the one with a rule attached: the spec caps what a page may
see at 8, and reports it in powers of two. A persona claiming 32 GB must still
answer 8, and answering 32 would be a value no real Chrome produces.

Usage: core/verify/verify-0010.py <core binary>
"""

import http.server
import json
import os
import sys
import threading

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

PERSONA = {
    "navigator": {
        "platform": "Win32",
        "deviceMemory": 8,
        "maxTouchPoints": 0,
        "languages": ["de-DE", "de", "en-US", "en"],
    }
}

READ = """
(async () => {
  const out = {
    platform: navigator.platform,
    deviceMemory: navigator.deviceMemory,
    maxTouchPoints: navigator.maxTouchPoints,
    languages: navigator.languages,
    language: navigator.language,
  };
  const w = new Worker(URL.createObjectURL(new Blob([
    'postMessage(JSON.stringify({platform: navigator.platform,' +
    ' languages: navigator.languages, deviceMemory: navigator.deviceMemory}))'])));
  out.worker = JSON.parse(await new Promise((r) => { w.onmessage = (e) => r(e.data); }));
  w.terminate();

  return JSON.stringify(out);
})()
"""


def serve_and_record():
    """A one-page server that keeps the headers of the request it answered.

    The first version fetched httpbin.org, which made a claim about this
    browser depend on somebody else's uptime — and it duly failed with
    "Failed to fetch" on the first run. A request header cannot be read from
    the page that sent it, so something has to receive it; that something may
    as well be local, and then the check is deterministic and works offline.
    """
    seen = {}

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            seen.update({k.lower(): v for k, v in self.headers.items()})
            body = b"<html><body>fury</body></html>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, seen


def main():
    claims = Claims("0010 — navigator basics and Accept-Language", CORE)
    server, headers = serve_and_record()
    page = f"http://127.0.0.1:{server.server_address[1]}/"

    # Loaded from the local server rather than example.com, so the navigation
    # itself carries the header being checked — no fetch, no mixed content, and
    # no third party.
    with launch(CORE, PERSONA, page=page) as s:
        got = json.loads(s.js(READ, timeout=60000))
        print(f"  measured: {got}")

        claims.check(got["platform"] == "Win32",
                     f"navigator.platform is Win32 (got {got['platform']!r})")
        claims.check(got["maxTouchPoints"] == 0,
                     f"maxTouchPoints is 0 (got {got['maxTouchPoints']})")
        claims.check(got["languages"] == PERSONA["navigator"]["languages"],
                     f"languages is the ordered list asked for (got {got['languages']})")
        claims.check(got["language"] == "de-DE",
                     f"and navigator.language is its first entry (got {got['language']!r})")

        # The spec caps this at 8 and reports powers of two; a browser that
        # answered anything else would be the odd one out.
        claims.check(got["deviceMemory"] in (0.25, 0.5, 1, 2, 4, 8),
                     f"deviceMemory is a value the spec allows (got {got['deviceMemory']})")

        claims.check(got["worker"]["platform"] == got["platform"]
                     and got["worker"]["languages"] == got["languages"],
                     f"a Worker agrees on all of it: {got['worker']}")

        # The one that is not a property. Built in //net from a preference
        # rather than from navigator.languages, so a patch that sets the
        # JavaScript value and stops leaves the browser announcing one language
        # over the wire and another to the page.
        header = headers.get("accept-language", "")
        print(f"  Accept-Language as the server saw it: {header!r}")
        claims.check(header.startswith("de-DE"),
                     f"Accept-Language leads with de-DE (got {header!r})")
        claims.check("en" in header,
                     "and carries the rest of the list, not just the first")

    with launch(CORE, None) as s:
        bare = json.loads(s.js(
            "JSON.stringify({p: navigator.platform, l: navigator.languages})"))
        print(f"  unconfigured: {bare}")
        claims.control(
            bare["p"] != "Win32" or bare["l"] != PERSONA["navigator"]["languages"],
            f"an unconfigured build reports the host's own values (got {bare})",
        )

    server.shutdown()
    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
