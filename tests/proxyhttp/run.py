#!/usr/bin/env python3
"""Run a real Formicary server and exercise Chamber-configured HTTP proxies."""

from __future__ import annotations

import http.client
import json
import os
from pathlib import Path
import signal
import shlex
import socket
import subprocess
import sys
import tempfile
import time


def reserve_local_ports(count: int) -> list[int]:
    listeners: list[socket.socket] = []
    try:
        for _ in range(count):
            listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            listener.bind(("127.0.0.1", 0))
            listeners.append(listener)
        return [listener.getsockname()[1] for listener in listeners]
    finally:
        for listener in listeners:
            listener.close()


def request(
    port: int,
    method: str,
    path: str,
    body: bytes | list[bytes] | None = None,
) -> tuple[int, dict[str, str], bytes]:
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=4)
    try:
        connection.request(method, path, body=body, encode_chunked=isinstance(body, list))
        response = connection.getresponse()
        result = response.status, {
            key.lower(): value for key, value in response.getheaders()
        }, response.read()
        return result
    finally:
        connection.close()


def service_is_ready(port: int) -> bool:
    try:
        status, _, body = request(port, "GET", "/api")
        return status == 200 and b"Formicary API" in body
    except (OSError, http.client.HTTPException):
        return False


def stop_service(process: subprocess.Popen[bytes] | None) -> None:
    if process is None or process.poll() is not None:
        return
    if os.name == "nt":
        process.terminate()
    else:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def stop_proxy_services(pid_files: list[Path]) -> None:
    if os.name == "nt":
        return
    process_groups: set[int] = set()
    for pid_file in pid_files:
        try:
            process_groups.add(int(pid_file.read_text(encoding="ascii")))
        except (OSError, ValueError):
            continue
    for process_group in process_groups:
        try:
            os.killpg(process_group, signal.SIGTERM)
        except ProcessLookupError:
            pass
    if process_groups:
        time.sleep(0.2)
    for process_group in process_groups:
        try:
            os.killpg(process_group, signal.SIGKILL)
        except ProcessLookupError:
            pass


def print_log_tail(log_path: Path) -> None:
    if not log_path.exists():
        return
    lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
    if lines:
        print("--- Formicary server log (last 40 lines) ---", file=sys.stderr)
        print("\n".join(lines[-40:]), file=sys.stderr)


def make_proxy_entry(
    name: str, route: str, port: int, pid_file: Path, base_path: str = ""
) -> str:
    script = shlex.join(["exec", "python3", "backend.py", str(port), name, str(pid_file)])
    return f'''[[proxyhttp]]
script = {json.dumps(script)}
upstream = "http://127.0.0.1:{port}{base_path}"
route = "{route}"
'''


def expect_json(
    port: int,
    method: str,
    path: str,
    expected: dict[str, str],
    body: bytes | list[bytes] | None = None,
) -> dict[str, str]:
    status, headers, response_body = request(port, method, path, body)
    if status != 200:
        raise RuntimeError(f"{method} {path} returned {status}; expected 200")
    if headers.get("x-backend") != expected["name"]:
        raise RuntimeError(
            f"{method} {path} returned X-Backend {headers.get('x-backend')!r}; "
            f"expected {expected['name']!r}"
        )
    if method == "HEAD":
        if response_body:
            raise RuntimeError(f"HEAD {path} unexpectedly returned a body")
        if int(headers.get("content-length", "0")) <= 0:
            raise RuntimeError(f"HEAD {path} did not preserve the upstream Content-Length")
        return {"name": expected["name"], "method": method, "path": expected["path"], "body": ""}
    try:
        actual = json.loads(response_body)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"{method} {path} returned invalid JSON: {response_body!r}") from error
    if actual != expected:
        raise RuntimeError(f"{method} {path} returned {actual!r}; expected {expected!r}")
    return actual


def expect_upstream_status(
    port: int, method: str, path: str, expected_status: int, expected_backend: str
) -> None:
    status, headers, _ = request(port, method, path)
    if status != expected_status:
        raise RuntimeError(
            f"{method} {path} returned {status}; expected upstream status {expected_status}"
        )
    if headers.get("x-backend") != expected_backend:
        raise RuntimeError(
            f"{method} {path} returned X-Backend {headers.get('x-backend')!r}; "
            f"expected {expected_backend!r}"
        )


def run(repo_root: Path) -> int:
    binary_name = "Formicary.exe" if os.name == "nt" else "Formicary"
    binary = repo_root / "target" / "debug" / binary_name
    test_directory = repo_root / "tests" / "proxyhttp"

    print("Building the Formicary server...")
    build = subprocess.run(["cargo", "build", "--bin", "Formicary"], cwd=repo_root)
    if build.returncode != 0:
        print("FAIL: cargo build did not complete successfully.", file=sys.stderr)
        return build.returncode
    if not binary.is_file():
        print(f"FAIL: built server binary was not found at {binary}.", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory(prefix=".runtime-", dir=test_directory) as temporary_root:
        temporary = Path(temporary_root)
        chamber_root = temporary / "chambers"
        chamber_root.mkdir()
        ports = reserve_local_ports(3)
        pid_files = [temporary / f"backend-{index}.pid" for index in range(3)]

        single = chamber_root / "single-table"
        single.mkdir()
        (single / "backend.py").write_bytes((test_directory / "backend.py").read_bytes())
        (single / "chamber.toml").write_text(
            f'''[chamber]
name = "single-table"
schema_version = 1

[proxyhttp]
script = {json.dumps(shlex.join(["exec", "python3", "backend.py", str(ports[0]), "single", str(pid_files[0])]))}
upstream = "http://127.0.0.1:{ports[0]}"
route = "/proxy"
''',
            encoding="utf-8",
        )

        multiple = chamber_root / "multiple-tables"
        multiple.mkdir()
        (multiple / "backend.py").write_bytes((test_directory / "backend.py").read_bytes())
        (multiple / "chamber.toml").write_text(
            f'''[chamber]
name = "multiple-tables"
schema_version = 1

{make_proxy_entry("one", "/one", ports[1], pid_files[1])}
{make_proxy_entry("two", "/two", ports[2], pid_files[2], "/v1")}''',
            encoding="utf-8",
        )

        port = reserve_local_ports(1)[0]
        environment = os.environ.copy()
        environment.update(
            {
                "FORMICARY_HOST": "127.0.0.1",
                "FORMICARY_PORT": str(port),
                "FORMICARY_DATABASE_PATH": str(temporary / "auth.sqlite3"),
                "FORMICARY_CHAMBERS_DIR": str(chamber_root),
            }
        )
        log_path = temporary / "server.log"
        process: subprocess.Popen[bytes] | None = None

        with log_path.open("wb") as server_log:
            try:
                process = subprocess.Popen(
                    [str(binary)],
                    cwd=test_directory,
                    env=environment,
                    stdout=server_log,
                    stderr=subprocess.STDOUT,
                    start_new_session=os.name != "nt",
                )
                print("Started isolated server; waiting for its HTTP endpoint...")
                ready_deadline = time.monotonic() + 15
                while time.monotonic() < ready_deadline:
                    if process.poll() is not None:
                        raise RuntimeError(
                            f"server exited during startup with code {process.returncode}"
                        )
                    if service_is_ready(port):
                        break
                    time.sleep(0.2)
                else:
                    raise RuntimeError("server did not become ready within 15 seconds")

                print("Server is ready; waiting for all configured upstream services...")
                for route in ("/proxy/health", "/one/health", "/two/health"):
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        try:
                            status, _, _ = request(port, "GET", route)
                            if status == 200:
                                break
                        except (OSError, http.client.HTTPException):
                            pass
                        time.sleep(0.1)
                    else:
                        raise RuntimeError(f"upstream for {route} did not become ready")

                print("Checking route stripping, query strings, bodies, and methods...")
                expect_json(
                    port,
                    "GET",
                    "/proxy/echo?value=one",
                    {"name": "single", "method": "GET", "path": "/echo?value=one", "body": ""},
                )
                expect_json(
                    port,
                    "POST",
                    "/one/submit?x=2",
                    {"name": "one", "method": "POST", "path": "/submit?x=2", "body": "payload"},
                    b"payload",
                )
                expect_json(
                    port,
                    "POST",
                    "/two/chunked",
                    {"name": "two", "method": "POST", "path": "/v1/chunked", "body": "chunked"},
                    [b"chunk", b"ed"],
                )
                expect_json(
                    port,
                    "GET",
                    "/two/chunked-response",
                    {
                        "name": "two",
                        "method": "GET",
                        "path": "/v1/chunked-response",
                        "body": "",
                    },
                )
                expect_upstream_status(port, "GET", "/proxy/teapot", 418, "single")
                expect_json(
                    port,
                    "HEAD",
                    "/proxy/head?value=2",
                    {"name": "single", "path": "/head?value=2"},
                )
                status, _, _ = request(port, "GET", "/proxy-extra/echo")
                if status != 404:
                    raise RuntimeError(f"unmatched /proxy-extra route returned {status}; expected 404")

                print(
                    "PASS: Chamber [proxyhttp] and [[proxyhttp]] entries started local services, "
                    "stripped route prefixes, preserved queries and bodies, decoded chunked responses, "
                    "and forwarded upstream status codes."
                )
                return 0
            except Exception as error:
                server_log.flush()
                print(f"FAIL: {error}", file=sys.stderr)
                print_log_tail(log_path)
                return 1
            finally:
                stop_service(process)
                stop_proxy_services(pid_files)


if __name__ == "__main__":
    root = (
        Path(sys.argv[1]).resolve()
        if len(sys.argv) > 1
        else Path(__file__).resolve().parents[2]
    )
    raise SystemExit(run(root))
