# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""A lightweight client for the Fury Local API.

Reads the authorization bearer token directly from disk on construction.
Uses only the Python standard library.
"""

from __future__ import annotations

import json
import os
import pathlib
import sys
import urllib.error
import urllib.request
from typing import Optional

DEFAULT_PORT = 35000


def _home() -> pathlib.Path:
    if "FURY_HOME" in os.environ:
        return pathlib.Path(os.environ["FURY_HOME"])
    if sys.platform == "win32":
        appdata = os.environ.get("APPDATA")
        if appdata:
            return pathlib.Path(appdata) / "Fury"
        return pathlib.Path.home() / "AppData" / "Roaming" / "Fury"
    if sys.platform == "darwin":
        return pathlib.Path.home() / "Library/Application Support/Fury"
    return pathlib.Path(os.environ.get("XDG_DATA_HOME", pathlib.Path.home() / ".local/share")) / "fury"


class Fury:
    """The agent's local API client."""

    def __init__(self, port: Optional[int] = None):
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

    def start(self, profile_id: str, cdp: bool = True, background: bool = False):
        return self._call("POST", "/profiles/start", {"id": profile_id, "cdp": cdp, "background": background})

    def stop(self, profile_id: str):
        return self._call("POST", "/profiles/stop", {"id": profile_id})


class running:
    """Context manager that stops the profile and releases locks on exit."""

    def __init__(self, fury: Fury, profile_id: str, cdp: bool = True, background: bool = False):
        self.fury = fury
        self.id = profile_id
        self.cdp = cdp
        self.background = background

    def __enter__(self):
        self.session = self.fury.start(self.id, cdp=self.cdp, background=self.background)
        return self.session

    def __exit__(self, *exc):
        try:
            self.fury.stop(self.id)
        except Exception as e:
            print(f"warning: could not stop profile {self.id}: {e}", flush=True)
        return False