#!/usr/bin/env python3
"""Small local HTTP service launched by the proxyhttp runtime smoke test."""

from __future__ import annotations

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import base64
import hashlib
import struct
import json
from pathlib import Path
import signal
import sys


port = int(sys.argv[1])
name = sys.argv[2]
pid_file = Path(sys.argv[3])
pid_file.write_text(str(__import__("os").getpid()), encoding="ascii")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:
        if self.headers.get("Upgrade", "").lower() == "websocket" and self.path.startswith(("/v1/ws", "/ws")):
            self.websocket()
        else:
            self.respond()

    def websocket(self) -> None:
        key = self.headers["Sec-WebSocket-Key"]
        accept = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        self.send_response(101)
        self.send_header("Connection", "Upgrade")
        self.send_header("Upgrade", "websocket")
        self.send_header("Sec-WebSocket-Accept", accept)
        self.send_header("Sec-WebSocket-Protocol", "echo")
        self.end_headers()
        def send(opcode, payload):
            length = len(payload)
            header = bytes([opcode, length]) if length < 126 else bytes([opcode, 126]) + struct.pack("!H", length) if length < 65536 else bytes([opcode, 127]) + struct.pack("!Q", length)
            self.wfile.write(header + payload)
            self.wfile.flush()
        # Send immediately after 101 to exercise bytes buffered during the upgrade.
        send(0x81, json.dumps({"path": self.path, "headers": dict(self.headers)}).encode())
        while True:
            header = self.rfile.read(2)
            if not header:
                break
            opcode, length = header
            assert length & 128
            length &= 127
            if length == 126:
                length = struct.unpack("!H", self.rfile.read(2))[0]
            elif length == 127:
                length = struct.unpack("!Q", self.rfile.read(8))[0]
            mask = self.rfile.read(4)
            payload = self.rfile.read(length)
            payload = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
            send(0x8A if opcode == 0x89 else opcode, payload)
            if opcode == 0x88:
                break
        self.close_connection = True

    def do_HEAD(self) -> None:
        self.respond(head_only=True)

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        self.request_body = self.rfile.read(length)
        self.respond()

    def respond(self, head_only: bool = False) -> None:
        payload = json.dumps(
            {
                "name": name,
                "method": self.command,
                "path": self.path,
                "body": getattr(self, "request_body", b"").decode("utf-8"),
            },
            separators=(",", ":"),
        ).encode("utf-8")
        status = 418 if self.path.split("?", 1)[0].endswith("/teapot") else 200
        chunked_response = self.path.split("?", 1)[0].endswith("/chunked-response")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("X-Backend", name)
        if chunked_response:
            self.send_header("Transfer-Encoding", "chunked")
        else:
            self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        if not head_only:
            if chunked_response:
                for offset in range(0, len(payload), 7):
                    chunk = payload[offset : offset + 7]
                    self.wfile.write(f"{len(chunk):X}\r\n".encode("ascii"))
                    self.wfile.write(chunk + b"\r\n")
                self.wfile.write(b"0\r\n\r\n")
            else:
                self.wfile.write(payload)

    def log_message(self, format: str, *args: object) -> None:
        pass


server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
