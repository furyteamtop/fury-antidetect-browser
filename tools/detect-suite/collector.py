#!/usr/bin/env python3
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
import socketserver
import sys

PORT = int(os.environ.get("PORT", "8731"))
HERE = pathlib.Path(__file__).resolve().parent
BASELINES = HERE / "baselines"

SAFE_NAME = re.compile(r"^[A-Za-z0-9._-]{1,120}$")


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(HERE), **kwargs)

    def do_POST(self):
        if self.path != "/save":
            self.send_error(404)
            return

        length = int(self.headers.get("Content-Length") or 0)
        if length <= 0 or length > 16 * 1024 * 1024:
            self.send_error(413, "dump missing or absurdly large")
            return

        try:
            payload = json.loads(self.rfile.read(length))
            name = payload["name"]
            dump = payload["dump"]
        except (ValueError, KeyError, TypeError) as exc:
            self.send_error(400, f"expected {{name, dump}}: {exc}")
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

        target.write_text(json.dumps(dump, indent=2, ensure_ascii=False))
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


if __name__ == "__main__":
    BASELINES.mkdir(exist_ok=True)
    with Server(("127.0.0.1", PORT), Handler) as httpd:
        print(f"probe   http://localhost:{PORT}/probe.html")
        print(f"saves   {BASELINES}")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            sys.exit(0)
