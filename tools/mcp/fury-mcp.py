#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""An MCP server for Fury, so an assistant can drive profiles.

    tools/mcp/fury-mcp.py            speak MCP on stdin/stdout
    tools/mcp/fury-mcp.py --selftest check the protocol without an agent

Point Claude Desktop or an IDE at it:

    {"mcpServers": {"fury": {"command": "/path/to/tools/mcp/fury-mcp.py"}}}

Standard library only, and that is the same decision `examples/fury.py` records
for the Python client: the surface underneath is six endpoints and one bearer
token, so a dependency to reach it would cost more than it saves. MCP over stdio
is newline-delimited JSON-RPC, which is thirty lines.

## The one thing this refuses to do

It exposes exactly the six endpoints the local HTTP API already has, and adds
nothing. Not because more would be hard, but because the HTTP API is the place
where "what automation may do" is decided — it is thin on purpose so it cannot
drift from what the desktop shell uses and be separately wrong. A second surface
with its own idea of what is allowed would be that drift, arriving through the
component most likely to be pointed at an assistant.

So: launching a profile is here, because it is there. Deleting one is not,
because it is not.
"""

from __future__ import annotations

import json
import os
import pathlib
import sys
import urllib.error
import urllib.request

DEFAULT_PORT = 35000
PROTOCOL = "2025-06-18"


# --------------------------------------------------------------------------
# Reaching the agent. The same discovery examples/fury.py does, for the same
# reason: the token is written by the agent into its data directory, and
# anything that guesses instead of reading it is a thing that breaks when
# FURY_HOME moves.
# --------------------------------------------------------------------------
def _home() -> pathlib.Path:
    if os.environ.get("FURY_HOME"):
        return pathlib.Path(os.environ["FURY_HOME"])
    if sys.platform == "darwin":
        return pathlib.Path.home() / "Library/Application Support/Fury"
    if sys.platform == "win32":
        base = os.environ.get("APPDATA") or (pathlib.Path.home() / "AppData/Roaming")
        return pathlib.Path(base) / "Fury"
    return pathlib.Path(
        os.environ.get("XDG_DATA_HOME", pathlib.Path.home() / ".local/share")
    ) / "fury"


def _token() -> str:
    path = _home() / "api-token"
    try:
        return path.read_text().strip()
    except OSError as e:
        raise RuntimeError(
            f"cannot read {path}: start the agent once so it writes its token "
            f"({e})"
        ) from e


def call(method: str, path: str, body: dict | None = None) -> dict:
    port = int(os.environ.get("FURY_PORT", DEFAULT_PORT))
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}",
        data=data,
        method=method,
        headers={
            "Authorization": f"Bearer {_token()}",
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        detail = e.read().decode(errors="replace")
        raise RuntimeError(f"the agent refused: HTTP {e.code} {detail}") from e
    except urllib.error.URLError as e:
        raise RuntimeError(
            f"nothing answered on 127.0.0.1:{port} — is the agent running? ({e.reason})"
        ) from e


# --------------------------------------------------------------------------
# The tools. One per endpoint, no more.
# --------------------------------------------------------------------------
NOTHING = {"type": "object", "properties": {}}
BY_ID = {
    "type": "object",
    "properties": {"id": {"type": "string", "description": "The profile's id."}},
    "required": ["id"],
}

TOOLS = [
    {
        "name": "fury_status",
        "description": "Whether the agent is up, which core it has, and what is running.",
        "inputSchema": NOTHING,
        "_call": lambda a: call("GET", "/v1/status"),
    },
    {
        "name": "fury_profiles",
        "description": "Every profile: id, name, persona, proxy, and whether it is open.",
        "inputSchema": NOTHING,
        "_call": lambda a: call("GET", "/v1/profiles"),
    },
    {
        "name": "fury_proxies",
        "description": "Every proxy, with the exit each was last seen leaving from.",
        "inputSchema": NOTHING,
        "_call": lambda a: call("GET", "/v1/proxies"),
    },
    {
        "name": "fury_personas",
        "description": "The device personas a profile can claim to be.",
        "inputSchema": NOTHING,
        "_call": lambda a: call("GET", "/v1/personas"),
    },
    {
        "name": "fury_start_profile",
        "description": (
            "Open a profile. Returns a CDP WebSocket endpoint when that profile "
            "was launched with debugging allowed — it is off unless asked for."
        ),
        "inputSchema": BY_ID,
        "_call": lambda a: call("POST", "/v1/profiles/start", {"id": a["id"]}),
    },
    {
        "name": "fury_stop_profile",
        "description": (
            "Close a profile. Asks the browser to quit rather than killing it, "
            "so the cookie jar is written."
        ),
        "inputSchema": BY_ID,
        "_call": lambda a: call("POST", "/v1/profiles/stop", {"id": a["id"]}),
    },
]

BY_NAME = {t["name"]: t for t in TOOLS}


def _advertised(tool: dict) -> dict:
    return {k: v for k, v in tool.items() if not k.startswith("_")}


# --------------------------------------------------------------------------
# JSON-RPC
# --------------------------------------------------------------------------
def handle(message: dict) -> dict | None:
    """One request in, one response out. `None` for a notification."""
    method = message.get("method")
    mid = message.get("id")

    # A notification has no id and must get no reply. Answering one is the
    # mistake that makes a client hang waiting for a response it will then not
    # match to anything.
    if mid is None:
        return None

    def ok(result: dict) -> dict:
        return {"jsonrpc": "2.0", "id": mid, "result": result}

    def err(code: int, msg: str) -> dict:
        return {"jsonrpc": "2.0", "id": mid, "error": {"code": code, "message": msg}}

    if method == "initialize":
        return ok(
            {
                "protocolVersion": PROTOCOL,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fury", "version": "0.1.0"},
            }
        )

    if method == "tools/list":
        return ok({"tools": [_advertised(t) for t in TOOLS]})

    if method == "tools/call":
        params = message.get("params") or {}
        name = params.get("name")
        tool = BY_NAME.get(name)
        if tool is None:
            return err(-32602, f"no tool called {name!r}")
        try:
            result = tool["_call"](params.get("arguments") or {})
        except Exception as e:  # noqa: BLE001 — the message is the product here
            # isError rather than a JSON-RPC error: the call was well-formed and
            # the agent said no, which the assistant should read and act on
            # rather than treat as a broken server.
            return ok(
                {
                    "content": [{"type": "text", "text": str(e)}],
                    "isError": True,
                }
            )
        return ok({"content": [{"type": "text", "text": json.dumps(result, indent=2)}]})

    return err(-32601, f"unknown method {method!r}")


def serve() -> int:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError as e:
            print(
                json.dumps(
                    {"jsonrpc": "2.0", "id": None,
                     "error": {"code": -32700, "message": f"parse error: {e}"}}
                ),
                flush=True,
            )
            continue
        response = handle(message)
        if response is not None:
            print(json.dumps(response), flush=True)
    return 0


# --------------------------------------------------------------------------
# The checks, because a protocol implementation nobody exercises is a
# protocol implementation that is wrong.
# --------------------------------------------------------------------------
def selftest() -> int:
    checks = []

    def claim(ok: bool, text: str) -> None:
        checks.append((ok, text))
        print(f"  {'OK  ' if ok else 'FAIL'} {text}")

    r = handle({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})
    claim(r["result"]["protocolVersion"] == PROTOCOL, "initialize names a protocol version")
    claim("tools" in r["result"]["capabilities"], "and advertises tools")

    r = handle({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    names = [t["name"] for t in r["result"]["tools"]]
    claim(len(names) == 6, f"six tools, one per endpoint (got {len(names)})")
    claim(
        all("_call" not in t for t in r["result"]["tools"]),
        "the python callable does not leak into the wire format",
    )
    claim(
        all("inputSchema" in t and "description" in t for t in r["result"]["tools"]),
        "every tool carries a schema and a description",
    )

    # A notification MUST NOT be answered. Answering one leaves a client
    # waiting for a reply it cannot match.
    claim(
        handle({"jsonrpc": "2.0", "method": "notifications/initialized"}) is None,
        "a notification gets no response",
    )

    r = handle({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "nope", "arguments": {}}})
    claim(r.get("error", {}).get("code") == -32602, "an unknown tool is a JSON-RPC error")

    r = handle({"jsonrpc": "2.0", "id": 4, "method": "sing"})
    claim(r.get("error", {}).get("code") == -32601, "an unknown method is -32601")

    # With no agent listening, a call has to come back as isError with
    # something readable — not as a crash, and not as a silent empty result.
    os.environ["FURY_PORT"] = "1"
    os.environ.setdefault("FURY_HOME", "/nonexistent")
    r = handle({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
                "params": {"name": "fury_status", "arguments": {}}})
    body = r["result"]
    claim(body.get("isError") is True, "a dead agent is isError rather than a crash")
    claim(
        "agent" in body["content"][0]["text"],
        f"and says so in words: {body['content'][0]['text'][:60]!r}",
    )

    failed = [t for ok, t in checks if not ok]
    print()
    if failed:
        print(f"FAIL — {len(failed)} of {len(checks)}")
        return 1
    print(f"PASS — {len(checks)} checks")
    return 0


if __name__ == "__main__":
    sys.exit(selftest() if "--selftest" in sys.argv else serve())
