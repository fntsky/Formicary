#!/usr/bin/env python3
"""Run a real scheduled Job in an isolated Formicary server process."""

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


def wait_for_cron_window() -> None:
    """Start at :28–:30 so a five-field every-minute Job fires about 30s later."""
    while True:
        seconds_into_minute = time.time() % 60
        if 28 <= seconds_into_minute < 31:
            return
        seconds_until_window = (
            28 - seconds_into_minute
            if seconds_into_minute < 28
            else 60 - seconds_into_minute + 28
        )
        time.sleep(min(seconds_until_window, 0.25))


def marker_lines(marker: Path) -> list[str]:
    if not marker.exists():
        return []
    return [line for line in marker.read_text(encoding="utf-8").splitlines() if line]


def service_is_ready(port: int) -> bool:
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
    try:
        connection.request("GET", "/api")
        response = connection.getresponse()
        body = response.read()
        return response.status == 200 and b"Formicary API" in body
    except (OSError, http.client.HTTPException):
        return False
    finally:
        connection.close()


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


def run(repo_root: Path) -> int:
    binary_name = "Formicary.exe" if os.name == "nt" else "Formicary"
    binary = repo_root / "target" / "debug" / binary_name
    test_directory = repo_root / "tests" / "job"

    print("Building the Formicary server...")
    build = subprocess.run(["cargo", "build", "--bin", "Formicary"], cwd=repo_root)
    if build.returncode != 0:
        print("FAIL: cargo build did not complete successfully.", file=sys.stderr)
        return build.returncode
    if not binary.is_file():
        print(f"FAIL: built server binary was not found at {binary}.", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory(
        prefix=".runtime-", dir=test_directory
    ) as temporary_root:
        temporary = Path(temporary_root)
        chamber_directory = temporary / "chambers" / "runtime-smoke"
        chamber_directory.mkdir(parents=True)
        marker = chamber_directory / "executions.log"
        config = chamber_directory / "chamber.toml"
        config.write_text(
            """[chamber]
name = "runtime-smoke"
schema_version = 1

[[jobs]]
name = "scheduled-marker"
cron = "* * * * *"
working_directory = "."
command = "echo fired >> executions.log"
timeout = "10s"
""",
            encoding="utf-8",
        )

        port = reserve_local_port()
        environment = os.environ.copy()
        environment.update(
            {
                "FORMICARY_HOST": "127.0.0.1",
                "FORMICARY_PORT": str(port),
                "FORMICARY_DATABASE_PATH": str(temporary / "auth.sqlite3"),
                "FORMICARY_CHAMBERS_DIR": str(temporary / "chambers"),
            }
        )
        log_path = temporary / "server.log"
        process: subprocess.Popen[bytes] | None = None

        with log_path.open("wb") as server_log:
            try:
                print("Aligning startup for a cron tick about 30 seconds later...")
                wait_for_cron_window()
                started_at = time.monotonic()
                process = subprocess.Popen(
                    [str(binary)],
                    cwd=test_directory,
                    env=environment,
                    stdout=server_log,
                    stderr=subprocess.STDOUT,
                )
                print("Started isolated server; waiting for its HTTP endpoint...")

                ready_deadline = time.monotonic() + 12
                while time.monotonic() < ready_deadline:
                    if process.poll() is not None:
                        raise RuntimeError(
                            f"server exited during startup with code {process.returncode}"
                        )
                    if service_is_ready(port):
                        break
                    time.sleep(0.2)
                else:
                    raise RuntimeError("server did not become ready within 12 seconds")

                early_check_deadline = started_at + 5
                while time.monotonic() < early_check_deadline:
                    if marker_lines(marker):
                        raise RuntimeError("Job ran immediately instead of waiting for its cron tick")
                    if process.poll() is not None:
                        raise RuntimeError(
                            f"server exited before the scheduled tick with code {process.returncode}"
                        )
                    time.sleep(0.1)

                print("Server is ready; verifying the scheduled shell command runs...")
                execution_deadline = started_at + 52
                while time.monotonic() < execution_deadline:
                    if process.poll() is not None:
                        raise RuntimeError(
                            f"server exited before Job execution with code {process.returncode}"
                        )
                    lines = marker_lines(marker)
                    if lines:
                        break
                    time.sleep(0.2)
                else:
                    raise RuntimeError("Job did not create its marker within 52 seconds")

                elapsed = time.monotonic() - started_at
                if not 20 <= elapsed <= 50:
                    raise RuntimeError(
                        f"first execution took {elapsed:.1f}s; expected it about 30s after startup"
                    )
                time.sleep(0.5)
                lines = marker_lines(marker)
                if lines != ["fired"]:
                    raise RuntimeError(f"expected one 'fired' marker, received {lines!r}")

                print(
                    f"PASS: scheduled Job executed its shell command after {elapsed:.1f}s "
                    "and wrote the expected marker."
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
    root = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[2]
    raise SystemExit(run(root))
