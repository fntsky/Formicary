#!/usr/bin/env python3
"""Run a real Formicary server and check Chamber-configured static routes."""

from __future__ import annotations

import http.client
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time


def reserve_local_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def request(port: int, method: str, path: str) -> tuple[int, dict[str, str], bytes]:
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
    try:
        connection.request(method, path)
        response = connection.getresponse()
        body = response.read()
        return response.status, {key.lower(): value for key, value in response.getheaders()}, body
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
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def print_log_tail(log_path: Path) -> None:
    if not log_path.exists():
        return
    lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
    if lines:
        print("--- Formicary server log (last 30 lines) ---", file=sys.stderr)
        print("\n".join(lines[-30:]), file=sys.stderr)


def expect_response(
    port: int,
    method: str,
    path: str,
    expected_status: int,
    expected_body: bytes | None = None,
    expected_content_type: str | None = None,
) -> None:
    status, headers, body = request(port, method, path)
    if status != expected_status:
        raise RuntimeError(f"{method} {path} returned {status}; expected {expected_status}")
    if expected_body is not None and body != expected_body:
        raise RuntimeError(f"{method} {path} returned unexpected body: {body!r}")
    if expected_content_type is not None:
        content_type = headers.get("content-type")
        if content_type != expected_content_type:
            raise RuntimeError(
                f"{method} {path} returned Content-Type {content_type!r}; "
                f"expected {expected_content_type!r}"
            )


def write_chamber(directory: Path, name: str, config: str) -> None:
    chamber_directory = directory / name
    chamber_directory.mkdir(parents=True)
    (chamber_directory / "chamber.toml").write_text(config, encoding="utf-8")


def run(repo_root: Path) -> int:
    binary_name = "Formicary.exe" if os.name == "nt" else "Formicary"
    binary = repo_root / "target" / "debug" / binary_name
    test_directory = repo_root / "tests" / "staticweb"

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

        single_chamber = chamber_root / "single-table"
        single_public = single_chamber / "public"
        single_public.mkdir(parents=True)
        (single_public / "index.html").write_text("single index\n", encoding="utf-8")
        (single_public / "about.html").write_text("about page\n", encoding="utf-8")
        single_config_directory = single_chamber
        (single_config_directory / "chamber.toml").write_text(
            """[chamber]
name = "single-table"
schema_version = 1

[staticweb]
directory = "./public"
route = "site"
""",
            encoding="utf-8",
        )

        multiple_chamber = chamber_root / "multiple-tables"
        (multiple_chamber / "assets").mkdir(parents=True)
        (multiple_chamber / "docs").mkdir()
        (multiple_chamber / "assets" / "app.js").write_text(
            "console.log('static');\n", encoding="utf-8"
        )
        (multiple_chamber / "docs" / "index.html").write_text(
            "docs index\n", encoding="utf-8"
        )
        (multiple_chamber / "chamber.toml").write_text(
            """[chamber]
name = "multiple-tables"
schema_version = 1

[[staticweb]]
directory = "./assets"
route = "/assets"

[[staticweb]]
directory = "./docs"
route = "/docs"
""",
            encoding="utf-8",
        )

        outside_file = temporary / "outside.html"
        outside_file.write_text("outside mount\n", encoding="utf-8")
        symlink_created = False
        try:
            (single_public / "leak.html").symlink_to(outside_file)
            symlink_created = True
        except (NotImplementedError, OSError):
            pass

        port = reserve_local_port()
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

                print("Server is ready; checking Chamber-loaded static routes...")
                expect_response(
                    port,
                    "GET",
                    "/site",
                    200,
                    b"single index\n",
                    "text/html; charset=utf-8",
                )
                expect_response(
                    port,
                    "GET",
                    "/site/about.html",
                    200,
                    b"about page\n",
                    "text/html; charset=utf-8",
                )
                expect_response(
                    port,
                    "GET",
                    "/assets/app.js",
                    200,
                    b"console.log('static');\n",
                    "text/javascript; charset=utf-8",
                )
                expect_response(
                    port,
                    "GET",
                    "/docs/",
                    200,
                    b"docs index\n",
                    "text/html; charset=utf-8",
                )
                expect_response(port, "HEAD", "/site/about.html", 200, b"")
                expect_response(port, "GET", "/site-extra/about.html", 404)
                expect_response(port, "GET", "/site/../outside.html", 404)
                if symlink_created:
                    expect_response(port, "GET", "/site/leak.html", 404)

                print(
                    "PASS: Chamber staticweb tables registered their routes and served "
                    "the expected files, headers, and path protections."
                )
                return 0
            except Exception as error:
                stop_service(process)
                server_log.flush()
                print(f"FAIL: {error}", file=sys.stderr)
                print_log_tail(log_path)
                return 1
            finally:
                stop_service(process)


if __name__ == "__main__":
    root = (
        Path(sys.argv[1]).resolve()
        if len(sys.argv) > 1
        else Path(__file__).resolve().parents[2]
    )
    raise SystemExit(run(root))
