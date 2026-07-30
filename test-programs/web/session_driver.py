"""Drive a recorded web session (RS-M5) — shared by the tests and the demo.

Everything here is real: a real server subprocess started by ``serve.py`` with
the recorder active, real HTTP over loopback, a real ``.ct`` container decoded by
the canonical Nim span reader.

Two users:

* ``tests/python/test_request_spans.py`` uses :class:`ServerUnderRecorder`
  directly so each test can choose its own request schedule (including
  concurrent ones).
* ``just demo-request-panel-python`` (and the codetracer-side fixture
  regeneration) runs this module as a script, which records
  :data:`DEMO_REQUESTS` — a schedule covering every status bucket the Request
  Panel colours — and prints the resulting spans.

Run it standalone with::

    python test-programs/web/session_driver.py --framework flask \\
        --trace-dir /tmp/ct-demo-python
"""

from __future__ import annotations

import argparse
import os
import signal
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import List, Optional, Sequence, Tuple

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parents[1]
SERVE_SCRIPT = HERE / "serve.py"

#: The demo request schedule: ``(method, path, body)``.
#:
#: Chosen so a panel opened on the resulting container shows something worth
#: looking at — every status bucket it colours (2xx, 3xx, 4xx, 5xx), four
#: methods, a duration on each side of the "instant" boundary, and a shared
#: ``/api/users`` URL prefix for the search box to narrow.
DEMO_REQUESTS: Sequence[Tuple[str, str, Optional[bytes]]] = (
    ("GET", "/api/users", None),
    ("POST", "/api/users", b'{"name":"Carol"}'),
    ("GET", "/api/users/2", None),
    ("GET", "/static/app.css", None),
    ("GET", "/api/users/999", None),
    ("GET", "/api/reports/slow", None),
    ("GET", "/api/boom", None),
    ("GET", "/api/users", None),
)


def free_port() -> int:
    """Reserve a loopback port by binding and releasing it."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class ServerUnderRecorder:
    """A real server subprocess, recorded, driven over real HTTP.

    The subprocess is ``serve.py``: it starts the recording, serves the chosen
    demo app, and on ``SIGTERM`` stops the recording so the container — span
    stream included — is written.  Leaving the context manager performs that
    shutdown and asserts the process exited cleanly.
    """

    def __init__(
        self,
        framework: str,
        trace_dir: Path,
        python: Optional[str] = None,
        threaded: bool = False,
        ready_timeout: float = 300.0,
        stop_timeout: float = 180.0,
        request_timeout: float = 60.0,
    ) -> None:
        self.framework = framework
        self.trace_dir = Path(trace_dir)
        self.python = python or sys.executable
        # WSGI only: thread-per-request serving, the way Flask and Django are
        # actually deployed.
        self.threaded = threaded
        # Every wait is bounded so a stuck server fails the caller instead of
        # hanging it.  The defaults are generous (a recorded server imports its
        # framework and starts a recording before it listens); a caller that
        # EXPECTS trouble should shorten them rather than wait the defaults out.
        self.ready_timeout = ready_timeout
        self.stop_timeout = stop_timeout
        self.request_timeout = request_timeout
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.process: Optional[subprocess.Popen] = None
        self.output: List[str] = []
        self._reader: Optional[threading.Thread] = None

    def __enter__(self) -> "ServerUnderRecorder":
        env = dict(os.environ)
        # Unbuffered, so the READY handshake is not stuck in a pipe buffer.
        env["PYTHONUNBUFFERED"] = "1"
        # A stray manifest env var from the developer's shell would re-enable the
        # sidecar this milestone removed from the recorded path.
        env.pop("CODETRACER_SPAN_MANIFEST", None)
        argv = [
            self.python,
            str(SERVE_SCRIPT),
            "--framework",
            self.framework,
            "--trace-dir",
            str(self.trace_dir),
            "--port",
            str(self.port),
        ]
        if self.threaded:
            argv.append("--threaded")
        self.process = subprocess.Popen(
            argv,
            cwd=str(REPO_ROOT),
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )

        ready = threading.Event()

        def drain() -> None:
            assert self.process is not None and self.process.stdout is not None
            for line in self.process.stdout:
                self.output.append(line.rstrip("\n"))
                if line.startswith("READY "):
                    ready.set()

        self._reader = threading.Thread(target=drain, name="serve-output", daemon=True)
        self._reader.start()

        # Generous: the server imports its framework and starts a recording
        # before it listens, and this suite runs alongside the rest of the
        # recorder's test suite on shared CI hardware.  A tight deadline turned
        # "slow" into a spurious failure once already.
        deadline = time.monotonic() + self.ready_timeout
        while not ready.is_set():
            if self.process.poll() is not None:
                raise AssertionError(
                    f"{self.framework} server exited before serving "
                    f"(rc={self.process.returncode}):\n" + self.log()
                )
            if time.monotonic() > deadline:
                # Ask the server to dump every thread's Python stack before it
                # dies: "never became ready" with no output is otherwise
                # undiagnosable, and `serve.py` registers SIGUSR1 for exactly
                # this.  The dump arrives on the same pipe as its stdout.
                try:
                    self.process.send_signal(signal.SIGUSR1)
                    time.sleep(2)
                except (OSError, ValueError):  # pragma: no cover - already dead
                    pass
                self.process.kill()
                self.process.wait(timeout=30)
                if self._reader is not None:
                    self._reader.join(timeout=5)
                raise AssertionError(
                    f"{self.framework} server never became ready in "
                    f"{self.ready_timeout}s; its threads were:\n" + self.log()
                )
            time.sleep(0.05)
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        assert self.process is not None
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
        try:
            self.process.wait(timeout=self.stop_timeout)
        except subprocess.TimeoutExpired:
            self.process.kill()
            raise AssertionError(
                f"{self.framework} server did not stop after SIGTERM:\n" + self.log()
            )
        if self._reader is not None:
            self._reader.join(timeout=10)
        if exc_type is None:
            assert self.process.returncode == 0, (
                f"{self.framework} server exited with {self.process.returncode}:\n" + self.log()
            )

    def log(self) -> str:
        return "\n".join(self.output)

    def request(
        self,
        path: str,
        method: str = "GET",
        body: Optional[bytes] = None,
    ) -> Tuple[int, bytes]:
        """Issue one real HTTP request and return ``(status, body)``.

        A 4xx / 5xx is a legitimate outcome (the schedules deliberately provoke
        both), so ``HTTPError`` is unwrapped into a status rather than raised.
        """
        request = urllib.request.Request(
            f"{self.base}{path}",
            data=body,
            method=method,
            headers={"Content-Type": "application/json"} if body else {},
        )
        try:
            with urllib.request.urlopen(request, timeout=self.request_timeout) as response:
                return response.status, response.read()
        except urllib.error.HTTPError as err:
            return err.code, err.read()

    def container(self) -> Path:
        """The single ``.ct`` container the recording produced."""
        containers = sorted(self.trace_dir.glob("*.ct"))
        assert containers, (
            f"no .ct container in {self.trace_dir}; contents="
            f"{sorted(p.name for p in self.trace_dir.iterdir())}\n" + self.log()
        )
        assert len(containers) == 1, f"expected one container, got {containers}"
        return containers[0]


def record_demo_session(framework: str, trace_dir: Path) -> Tuple[Path, List[int]]:
    """Record :data:`DEMO_REQUESTS` against *framework* into *trace_dir*.

    Returns the container path and the HTTP statuses observed, so a caller can
    report what the session actually served.
    """
    statuses: List[int] = []
    with ServerUnderRecorder(framework, trace_dir) as server:
        for method, path, body in DEMO_REQUESTS:
            status, _ = server.request(path, method=method, body=body)
            statuses.append(status)
    return server.container(), statuses


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--framework", default="flask", choices=("flask", "django", "fastapi"))
    parser.add_argument("--trace-dir", required=True)
    parser.add_argument(
        "--print-spans",
        action="store_true",
        help="decode and print the recorded span stream when done",
    )
    args = parser.parse_args(argv)

    trace_dir = Path(args.trace_dir)
    container, statuses = record_demo_session(args.framework, trace_dir)
    print(f"recorded {len(statuses)} requests -> {container}")

    if args.print_spans:
        from codetracer_python_recorder.spans import read_span_stream

        for span in read_span_stream(str(container)):
            metadata = dict(span["metadata"])
            print(
                f"  span {span['span_id']:>3}  {span['label']:<28}"
                f" status={metadata.get('http.status_code', '?'):>3}"
                f" {metadata.get('http.duration_ms', '?'):>5}ms"
                f" steps {span['start_step']}..{span['end_step']}"
                f" route={metadata.get('http.route', '-')}"
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
