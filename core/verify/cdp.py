"""A CDP client small enough to trust.

No dependency on `websockets` or node — neither is on this machine, and pulling
one in for a verification script would mean the verification depends on
something the thing being verified does not. The protocol here is the subset a
localhost DevTools socket actually uses: an HTTP upgrade, then text frames,
masked outbound and unmasked in.
"""

import base64
import json
import os
import socket
import struct
import urllib.request


class WS:
    def __init__(self, url, timeout=25.0):
        assert url.startswith("ws://"), url
        rest = url[len("ws://"):]
        hostport, _, path = rest.partition("/")
        host, _, port = hostport.partition(":")
        self.sock = socket.create_connection((host, int(port or 80)), timeout=timeout)
        self.sock.settimeout(timeout)
        key = base64.b64encode(os.urandom(16)).decode()
        req = (
            f"GET /{path} HTTP/1.1\r\n"
            f"Host: {hostport}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(req.encode())
        buf = b""
        while b"\r\n\r\n" not in buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("socket closed during handshake")
            buf += chunk
        if b"101" not in buf.split(b"\r\n")[0]:
            raise RuntimeError(f"upgrade refused: {buf.split(b'@@@')[0][:200]!r}")
        self.rest = buf.split(b"\r\n\r\n", 1)[1]
        self.next_id = 0

    # --- framing ------------------------------------------------------------
    def _recv(self, n):
        while len(self.rest) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise RuntimeError("socket closed")
            self.rest += chunk
        out, self.rest = self.rest[:n], self.rest[n:]
        return out

    def send(self, text):
        payload = text.encode()
        mask = os.urandom(4)
        n = len(payload)
        if n < 126:
            header = struct.pack("!BB", 0x81, 0x80 | n)
        elif n < 65536:
            header = struct.pack("!BBH", 0x81, 0x80 | 126, n)
        else:
            header = struct.pack("!BBQ", 0x81, 0x80 | 127, n)
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(header + mask + masked)

    def recv(self):
        while True:
            b0, b1 = self._recv(2)
            opcode = b0 & 0x0F
            n = b1 & 0x7F
            if n == 126:
                n = struct.unpack("!H", self._recv(2))[0]
            elif n == 127:
                n = struct.unpack("!Q", self._recv(8))[0]
            data = self._recv(n)
            if opcode == 0x8:  # close
                raise RuntimeError("peer closed")
            if opcode == 0x9:  # ping -> pong
                self.sock.sendall(struct.pack("!BB", 0x8A, 0x80 | len(data)) + b"\0\0\0\0" + data)
                continue
            if opcode in (0x1, 0x2):
                return data.decode()

    # --- CDP ----------------------------------------------------------------
    def call(self, method, params=None, session=None):
        self.next_id += 1
        wanted = self.next_id
        msg = {"id": wanted, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        self.send(json.dumps(msg))
        while True:
            reply = json.loads(self.recv())
            if reply.get("id") == wanted:
                if "error" in reply:
                    raise RuntimeError(f"{method}: {reply['error']}")
                return reply.get("result", {})

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


def browser(port):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version", timeout=15) as r:
        return WS(json.load(r)["webSocketDebuggerUrl"])
