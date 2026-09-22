#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Serve probe.html and accept dumps posted back to disk.

Why this exists: capturing a baseline means opening probe.html inside the browser
under test — real Chrome, AdsPower, Fury — and getting the JSON out. Downloading
it by hand and moving the file works but is slow and error-prone when you are
capturing a dozen of them, and browsers under test often have downloads locked
down. So the page POSTs its dump straight into ./baselines/.

    python3 tools/detect-suite/collector.py
    # then open http://localhost:8731/probe.html in the browser you are testing

Binds to loopback only. Refuses paths outside ./baselines/.
"""

import http.server
import json
import os
import pathlib
import re
import socket
import socketserver
import sys
import threading

PORT = int(os.environ.get("PORT", "8731"))
HERE = pathlib.Path(__file__).resolve().parent
BASELINES = HERE / "baselines"

SAFE_NAME = re.compile(r"^[A-Za-z0-9._-]{1,120}$")


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(HERE), **kwargs)

    def do_GET(self):
        # Client Hints arrive as request headers, and the whole point of patch
        # 0011 is that they agree with what navigator.userAgentData reports in
        # JS. Only the server side can see the headers, so record them here and
        # let the caller compare. Low-entropy hints are sent without an
        # Accept-CH negotiation, which is enough for the check.
        if self.path.startswith("/probe.html"):
            interesting = {
                k: v for k, v in self.headers.items()
                if k.lower().startswith("sec-ch-ua") or k.lower() == "user-agent"
                or k.lower() == "accept-language"
            }
            (BASELINES / "_last_request_headers.json").write_text(
                json.dumps(interesting, indent=2, ensure_ascii=False),
                encoding="utf-8",
            )
        # A ServiceWorker script has to be same-origin and cannot be a blob:,
        # which is why that context reported `__absent: "TypeError"` in every
        # capture until now — a whole execution context the headline number was
        # silently not covering. Served from here, registration succeeds and the
        # comparison finally includes it.
        #
        # Service-Worker-Allowed widens the scope so the script can control the
        # page that registered it whatever path it was fetched from.
        if self.path.startswith("/sw-probe.js"):
            body = (HERE / "sw-probe.js").read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", "application/javascript")
            self.send_header("Service-Worker-Allowed", "/")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        return super().do_GET()

    def do_POST(self):
        if self.path != "/save":
            self.send_error(404)
            return

        length = int(self.headers.get("Content-Length") or 0)
        if length <= 0 or length > 16 * 1024 * 1024:
            self.send_error(413, "dump missing or absurdly large")
            return

        raw = self.rfile.read(length)

        # Some clients leave the header block's terminating CRLF in the stream,
        # so a read of exactly Content-Length starts two bytes early and ends
        # two bytes short -- a body that is complete on the wire arrives as
        # `\r\n{...` with its last brace missing, and json fails at the final
        # character with "Expecting ',' delimiter". That reads exactly like
        # truncation and is not. Measured 10.09.2026 on a Fury profile posting
        # through the relay; the same POST direct from a bare browser is clean.
        body = raw.lstrip(b"\r\n")
        if len(body) < length:
            body += self.rfile.read(length - len(body))
        raw = body

        try:
            payload = json.loads(raw)
            name = payload["name"]
            dump = payload["dump"]
        except (ValueError, KeyError, TypeError) as exc:
            # Keep the body that failed. A 400 that discards it leaves a capture
            # that "did not answer" and nothing to look at -- the browser has
            # already been closed by the time anyone reads the log, and the dump
            # cannot be produced again without another launch.
            evidence = BASELINES / "_rejected_body.bin"
            evidence.write_bytes(raw)
            self.send_error(
                400,
                f"expected {{name, dump}}: {exc} "
                f"({len(raw)} of {length} bytes read, kept in {evidence.name})",
            )
            return

        # The name comes from the page, so treat it as untrusted: no traversal,
        # no absolute paths, no surprises.
        if not name.endswith(".json"):
            name += ".json"
        if not SAFE_NAME.match(name):
            self.send_error(400, "name must match [A-Za-z0-9._-]")
            return

        BASELINES.mkdir(exist_ok=True)
        target = (BASELINES / name).resolve()
        if BASELINES.resolve() not in target.parents:
            self.send_error(400, "path escapes baselines/")
            return

        # encoding="utf-8" explicitly, and it is not a nicety. write_text with no
        # encoding uses the platform default: UTF-8 on macOS, cp1252 on the
        # Windows build box. ensure_ascii=False above means the dump keeps the
        # characters it measured -- font family names, script samples -- and
        # cp1252 cannot encode them. The capture then leaves a ZERO-BYTE
        # baseline and a UnicodeEncodeError in collector.log, while the browser
        # has already been closed and the probe looks like it never answered.
        # Measured 10.09.2026, capturing the Chrome 153 reference on Windows.
        target.write_text(
            json.dumps(dump, indent=2, ensure_ascii=False), encoding="utf-8"
        )
        rel = target.relative_to(HERE.parent.parent)
        print(f"saved {rel} ({target.stat().st_size:,} bytes)", flush=True)

        body = json.dumps({"saved": str(rel)}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        # Log everything. An earlier version suppressed non-POST lines, which
        # also hid send_error() paths — a rejected POST left no trace at all and
        # "the dump never arrived" was indistinguishable from "the browser never
        # asked". Never make a diagnostic channel selective.
        super().log_message(fmt, *args)


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


class Server6(Server):
    address_family = socket.AF_INET6


if __name__ == "__main__":
    BASELINES.mkdir(exist_ok=True)
    # Both loopbacks, not one. The probe's cross-origin frame is the same page
    # on the OTHER loopback name — 127.0.0.1 asks localhost and vice versa —
    # and on macOS `localhost` resolves to ::1 first. A collector listening on
    # v4 alone answered the page and never saw the frame, which recorded the
    # ninth context as a load failure of the harness rather than the browser.
    # Loopback only, still: a listener on `::` would take the LAN too.
    v4 = Server(("127.0.0.1", PORT), Handler)
    try:
        v6 = Server6(("::1", PORT), Handler)
    except OSError as e:
        print(f"no IPv6 loopback ({e}); localhost may not reach the collector")
        v6 = None
    with v4:
        print(f"probe   http://127.0.0.1:{PORT}/probe.html")
        print(f"saves   {BASELINES}")
        if v6 is not None:
            threading.Thread(target=v6.serve_forever, daemon=True).start()
        try:
            v4.serve_forever()
        except KeyboardInterrupt:
            sys.exit(0)
