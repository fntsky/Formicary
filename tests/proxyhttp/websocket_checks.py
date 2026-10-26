"""WebSocket checks using only Python's standard library."""
import base64
import hashlib
import http.client
import signal
import sqlite3
import json
import os
import socket
import struct
import time


def connect(port, path="/two/ws?value=1", extra=""):
    sock = socket.create_connection(("127.0.0.1", port), timeout=5)
    key = base64.b64encode(os.urandom(16)).decode()
    sock.sendall((f"GET {path} HTTP/1.1\r\nHost: localhost\r\n"
                  f"Connection: keep-alive, Upgrade\r\nUpgrade: websocket\r\n"
                  f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
                  f"Sec-WebSocket-Protocol: echo\r\n{extra}\r\n").encode())
    reader = sock.makefile("rb")
    status = int(reader.readline().split()[1])
    headers = {}
    while (line := reader.readline()) != b"\r\n":
        assert line, "connection closed during handshake"
        name, value = line.decode().split(":", 1)
        headers[name.lower()] = value.strip()
    if status == 101:
        expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        assert headers["sec-websocket-accept"] == expected
        assert headers["sec-websocket-protocol"] == "echo"
        assert "content-length" not in headers
    return sock, reader, status, headers


def send(sock, opcode, payload):
    length = len(payload)
    size = bytes([length | 128]) if length < 126 else bytes([254]) + struct.pack("!H", length) if length < 65536 else bytes([255]) + struct.pack("!Q", length)
    mask = os.urandom(4)
    sock.sendall(bytes([opcode]) + size + mask + bytes(value ^ mask[index % 4] for index, value in enumerate(payload)))


def receive(reader):
    header = reader.read(2)
    assert len(header) == 2, "WebSocket closed unexpectedly"
    opcode, length = header
    assert not length & 128
    if length == 126:
        length = struct.unpack("!H", reader.read(2))[0]
    elif length == 127:
        length = struct.unpack("!Q", reader.read(8))[0]
    payload = reader.read(length)
    assert len(payload) == length
    return opcode, payload


def run_checks(port, database):
    # Seed credentials only in the isolated test database.
    key_id, secret = os.urandom(12).hex(), os.urandom(32)
    token = f"fmk_{key_id}_{secret.hex()}"
    secret_hash = hashlib.sha256(b"formicary.api-key.v1\0" + key_id.encode() + b"\0" + secret).digest()
    with sqlite3.connect(database) as db:
        user_id = db.execute("SELECT id FROM users WHERE username = 'admin'").fetchone()[0]
        db.execute("INSERT INTO sessions (token, user_id, expires_at) VALUES (?, ?, ?)", ("ws-test-session", user_id, int(time.time()) + 3600))
        db.execute("INSERT INTO api_keys (user_id, name, key_id, secret_hash, created_at) VALUES (?, 'ws-test', ?, ?, ?)", (user_id, key_id, secret_hash, int(time.time())))
    for extra, auth in [
        ("Cookie: formicary_session=ws-test-session\r\n", "session"),
        (f"Authorization: Bearer {token}\r\nCookie: formicary_session=ws-test-session; app=value\r\n", "api_key"),
    ]:
        sock, reader, status, response_headers = connect(port, extra=extra)
        try:
            assert status == 101
            info = json.loads(receive(reader)[1])
            headers = {key.lower(): value for key, value in info["headers"].items()}
            assert headers["x-formicary-user"] == "admin"
            assert headers["x-formicary-auth"] == auth
            if auth == "api_key":
                assert headers["x-formicary-key-id"] == key_id
                assert headers["x-formicary-principal"] == f"api-key:{key_id}"
                assert headers["cookie"] == "app=value"
                assert "authorization" not in headers
            send(sock, 0x88, b"")
            assert receive(reader) == (0x88, b"")
        finally:
            reader.close()
            sock.close()

    sock, reader, status, response_headers = connect(port, extra="Connection: keep-alive\r\nX-Formicary-User: spoofed\r\nCookie: app=value\r\nOrigin: http://localhost\r\n")
    try:
        assert status == 101
        opcode, payload = receive(reader)
        info = json.loads(payload)
        assert info["path"] == "/v1/ws?value=1"
        headers = {key.lower(): value for key, value in info["headers"].items()}
        assert "x-formicary-user" not in headers
        assert headers["cookie"] == "app=value"
        assert headers["origin"] == "http://localhost"
        for opcode, payload in [(0x81, "你好".encode()), (0x82, bytes(range(256)) * 300), (0x01, b"fragment"), (0x80, b"end"), (0x89, b"ping")]:
            send(sock, opcode, payload)
            assert receive(reader) == (0x8A if opcode == 0x89 else opcode, payload)
        # Ordinary HTTP has a 30-second total timeout; upgraded streams must survive it.
        time.sleep(31)
        send(sock, 0x81, b"still alive")
        assert receive(reader) == (0x81, b"still alive")
        send(sock, 0x88, struct.pack("!H", 1000))
        assert receive(reader) == (0x88, struct.pack("!H", 1000))
    finally:
        reader.close()
        sock.close()
    for path, extra, expected in [
        ("/proxy/teapot", "", 418),
        ("/two/ws", "X-API-Key: invalid\r\n", 401),
        ("/two-extra/ws", "", 404),
    ]:
        sock, reader, status, response_headers = connect(port, path, extra)
        try:
            assert status == expected, (path, status)
            if status == 418:
                # Read the chunked rejection without waiting for a keep-alive EOF.
                if "content-length" in response_headers:
                    payload = reader.read(int(response_headers["content-length"]))
                else:
                    payload = bytearray()
                    while (length := int(reader.readline().strip(), 16)):
                        payload.extend(reader.read(length))
                        assert reader.read(2) == b"\r\n"
                    assert reader.readline() == b"\r\n"
                assert b'"path":"/teapot"' in payload
        finally:
            reader.close()
            sock.close()

    # Stopping the Chamber must cancel an active tunnel and release its mount.
    sock, reader, status, response_headers = connect(port)
    try:
        assert status == 101
        receive(reader)
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            connection.request("POST", "/api/chambers/multiple-tables/stop", headers={"Cookie": "formicary_session=ws-test-session"})
            response = connection.getresponse()
            assert response.status == 200, response.read()
            response.read()
        finally:
            connection.close()
        assert reader.read(1) == b"", "unmounted tunnel stayed open"
    finally:
        reader.close()
        sock.close()


def run_shutdown_check(port, process):
    sock, reader, status, _ = connect(port, "/proxy/ws")
    try:
        assert status == 101
        receive(reader)
        process.send_signal(signal.SIGINT)
        assert reader.read(1) == b"", "shutdown left tunnel open"
        assert process.wait(timeout=5) == 0
    finally:
        reader.close()
        sock.close()
