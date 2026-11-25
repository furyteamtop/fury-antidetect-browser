# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov, devrahmanbd and the Fury authors
"""Zero-Dependency RFC 6455 WebSocket Client for Chrome DevTools Protocol (CDP).

Connects directly to Chromium's DevTools WebSocket endpoint using only
the Python standard library. No websockets, playwright, or external dependencies.
"""

from __future__ import annotations

import base64
import json
import os
import socket
import struct
import time
import urllib.parse
from typing import Any, Dict, Optional


class CDPClient:
    """Lightweight, synchronous WebSocket client implementing RFC 6455 framing."""

    def __init__(self, ws_url: str, timeout: float = 30.0):
        parsed = urllib.parse.urlparse(ws_url)
        host = parsed.hostname or "127.0.0.1"
        port = parsed.port or 9222
        path = parsed.path or "/"

        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.sock.settimeout(timeout)

        # RFC 6455 handshake
        key = base64.b64encode(os.urandom(16)).decode("ascii")
        req = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {host}:{port}\r\n"
            f"Upgrade: websocket\r\n"
            f"Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            f"Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(req.encode("ascii"))

        # Read handshake response
        buf = b""
        while b"\r\n\r\n" not in buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise ConnectionError("Socket closed during WebSocket handshake")
            buf += chunk

        status_line = buf.split(b"\r\n")[0]
        if b"101" not in status_line:
            raise ConnectionError(f"WebSocket upgrade rejected: {status_line.decode('utf-8', errors='replace')}")

        self.rest = buf.split(b"\r\n\r\n", 1)[1]
        self.next_id = 0

    def _recv_exact(self, num_bytes: int) -> bytes:
        while len(self.rest) < num_bytes:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("WebSocket peer closed connection")
            self.rest += chunk
        out, self.rest = self.rest[:num_bytes], self.rest[num_bytes:]
        return out

    def send(self, payload_text: str) -> None:
        """Sends a masked text frame (RFC 6455 client requirement)."""
        data = payload_text.encode("utf-8")
        mask = os.urandom(4)
        length = len(data)

        if length < 126:
            header = struct.pack("!BB", 0x81, 0x80 | length)
        elif length < 65536:
            header = struct.pack("!BBH", 0x81, 0x80 | 126, length)
        else:
            header = struct.pack("!BBQ", 0x81, 0x80 | 127, length)

        masked_data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        self.sock.sendall(header + mask + masked_data)

    def recv(self) -> str:
        """Receives and decodes the next text frame."""
        while True:
            b0, b1 = self._recv_exact(2)
            opcode = b0 & 0x0F
            length = b1 & 0x7F

            if length == 126:
                length = struct.unpack("!H", self._recv_exact(2))[0]
            elif length == 127:
                length = struct.unpack("!Q", self._recv_exact(8))[0]

            frame_data = self._recv_exact(length)

            if opcode == 0x8:  # Close frame
                raise ConnectionError("WebSocket closed by peer (opcode 0x8)")
            if opcode == 0x9:  # Ping -> reply Pong
                self.sock.sendall(struct.pack("!BB", 0x8A, 0x80 | len(frame_data)) + b"\0\0\0\0" + frame_data)
                continue
            if opcode in (0x1, 0x2):  # Text or binary
                return frame_data.decode("utf-8", errors="replace")

    def send_cmd(self, method: str, params: Optional[Dict[str, Any]] = None) -> None:
        """Sends a CDP command asynchronously without blocking for an RPC response.

        Ideal for streaming continuous mouse moves and wheel ticks where waiting
        for a synchronous round-trip ACK causes stalls if the window is occluded or minimized.
        """
        self.next_id += 1
        msg = {"id": self.next_id, "method": method, "params": params or {}}
        self.send(json.dumps(msg))

    def call(self, method: str, params: Optional[Dict[str, Any]] = None, timeout: float = 30.0) -> Dict[str, Any]:
        """Sends a CDP request and blocks until the corresponding ID response is received."""
        self.next_id += 1
        msg_id = self.next_id
        msg = {"id": msg_id, "method": method, "params": params or {}}
        self.send(json.dumps(msg))

        start_time = time.time()
        while time.time() - start_time < timeout:
            raw = self.recv()
            try:
                reply = json.loads(raw)
            except Exception:
                continue

            if reply.get("id") == msg_id:
                if "error" in reply:
                    raise RuntimeError(f"CDP {method} error: {reply['error']}")
                return reply.get("result", {})

        raise TimeoutError(f"CDP call {method} timed out after {timeout}s")

    def eval(self, js_expression: str, timeout: float = 20.0) -> Any:
        """Evaluates JavaScript expression in the main frame and returns the value.

        Runtime.evaluate does not require calling Runtime.enable, keeping the V8
        inspector debugging domain uninstrumented.
        """
        res = self.call(
            "Runtime.evaluate",
            {
                "expression": js_expression,
                "returnByValue": True,
                "awaitPromise": True,
            },
            timeout=timeout,
        )
        result_obj = res.get("result", {})
        if "value" in result_obj:
            return result_obj["value"]
        return None

    def close(self) -> None:
        try:
            self.sock.close()
        except Exception:
            pass