# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""A twenty-line client for the Fury automation API.

Deliberately not a package, not on PyPI, and not clever: the API is six
endpoints and one bearer token, and a dependency for that would be a worse deal
than copying this file into your project, which is what it is for.

Uses only the standard library, so it runs anywhere python3 does.
"""

# `int | None` in an annotation is Python 3.10, and macOS ships 3.9. This makes
# annotations strings so they are never evaluated, which is the difference
# between an example that runs on a stock Mac and one that does not.
from __future__ import annotations

import json
import os
import pathlib
import urllib.error
import urllib.request

DEFAULT_PORT = 35000


def _home() -> pathlib.Path:
    if "FURY_HOME" in os.environ:
        return pathlib.Path(os.environ["FURY_HOME"])
    if os.uname().sysname == "Darwin":
        return pathlib.Path.home() / "Library/Application Support/Fury"
    return pathlib.Path(os.environ.get("XDG_DATA_HOME", pathlib.Path.home() / ".local/share")) / "fury"


class Fury:
    """The agent's local API.

    The token is read from disk on every construction rather than passed in.
    It authorises every logged-in profile on the machine, and the surest way to
    leak one is to make it a constructor argument that ends up in a script.
    """

    def __init__(self, port: int | None = None):
        self.port = port or int(os.environ.get("FURY_API_PORT") or DEFAULT_PORT)
        token_file = _home() / "api-token"
        try:
            self.token = token_file.read_text().strip()
        except FileNotFoundError:
            raise SystemExit(
                f"no {token_file}\n"
                f"The agent writes it the first time it serves the API:\n"
                f"  FURY_API_PORT={self.port} fury-agent serve"
            )

    def _call(self, method: str, path: str, body: dict | None = None):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(
            f"http://127.0.0.1:{self.port}/v1{path}",
            data=data,
            method=method,
            headers={
                "Authorization": f"Bearer {self.token}",
                **({"Content-Type": "application/json"} if data else {}),
            },
        )
        try:
            with urllib.request.urlopen(req, timeout=120) as resp:
                return json.load(resp)["data"]
        except urllib.error.HTTPError as e:
            # The agent's own message is more useful than the status line: it
            # says "this profile has no proxy" rather than "400 Bad Request".
            detail = json.load(e).get("message", e.reason)
            raise RuntimeError(f"{path}: {detail}") from None
        except urllib.error.URLError as e:
            raise SystemExit(
                f"could not reach the agent on port {self.port} ({e.reason}).\n"
                f"Start it with: FURY_API_PORT={self.port} fury-agent serve"
            ) from None

    def status(self):
        return self._call("GET", "/status")

    def profiles(self):
        return self._call("GET", "/profiles")

    def proxies(self):
        return self._call("GET", "/proxies")

    def personas(self):
        return self._call("GET", "/personas")

    def start(self, profile_id: str, cdp: bool = False):
        return self._call("POST", "/profiles/start", {"id": profile_id, "cdp": cdp})

    def stop(self, profile_id: str):
        return self._call("POST", "/profiles/stop", {"id": profile_id})


class running:
    """`with running(fury, id) as session:` — stops the profile whatever happens.

    Worth having as a context manager rather than a start/stop pair, because the
    failure it prevents is expensive: a script that raises between them leaves
    the browser open, holding the profile's lock. On a team server that lock is
    what stops a colleague opening the same account in a second place, so a
    leaked one blocks a person rather than a process.
    """

    def __init__(self, fury: Fury, profile_id: str, cdp: bool = True):
        self.fury, self.id, self.cdp = fury, profile_id, cdp

    def __enter__(self):
        self.session = self.fury.start(self.id, cdp=self.cdp)
        return self.session

    def __exit__(self, *exc):
        try:
            self.fury.stop(self.id)
        except Exception as e:
            # Never mask the original exception with a cleanup failure — the
            # first one is the one worth reading.
            print(f"warning: could not stop {self.id}: {e}")
        return False
