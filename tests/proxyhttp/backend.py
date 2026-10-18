#!/usr/bin/env python3
"""Small local HTTP service launched by the proxyhttp runtime smoke test."""

from __future__ import annotations

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
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
        self.respond()

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
