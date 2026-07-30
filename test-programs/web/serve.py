"""Serve one of the web demo apps UNDER THE RECORDER (RS-M5).

The single entry point used by both the request-span integration tests and
``just demo-request-panel-python``:

    python test-programs/web/serve.py --framework flask \
        --trace-dir /tmp/flask-session --port 18901

It starts a recording, serves the chosen demo app over real HTTP until it is
asked to stop, then stops the recording so the ``.ct`` container — span stream
included — is written to disk.

Five details matter:

1. **A trace filter narrows the recording to the demo app module**
   (``trace_filter.toml`` next to this file).  Recording Flask / Django /
   uvicorn internals line-by-line would produce hundreds of megabytes and step
   ranges dominated by framework frames.
2. **The recorded timeline therefore contains request handling and nothing
   else**, which is what makes a span's step range meaningful.
3. **The app is imported before the recording starts**, so the framework's own
   import cost is not recorded (it is startup noise, and recording it is slow).
   Handlers are still traced: `sys.monitoring` events are enabled per tool, not
   per code object.  For the ASGI path this includes uvicorn's own lazily
   imported protocol implementation, which is why the server is fully
   constructed (`config.load()`) before the recording starts.
4. **Shutdown is graceful and explicit.**  ``SIGTERM`` (or ``SIGINT``) stops the
   server loop and then calls ``codetracer.stop()``; the container is only
   written at stop, so a killed process yields no trace.  ``READY <port>`` is
   printed once the socket is listening, so callers wait on that line instead of
   sleeping — the socket is bound *before* the recording starts, so a client that
   connects immediately waits in the backlog and is still recorded.
5. **No threads.**  Both serve loops run on the main thread, because the recorder
   cannot reliably trace a program that starts a thread (see
   ``test_threaded_wsgi_requests_land_in_span_stream``): a serving thread created
   after the recording started hung this harness at ``Thread.start()`` before it
   served anything.  It also makes the recorded step ranges deterministic.
"""

from __future__ import annotations

import argparse
import faulthandler
import importlib.util
import signal
import socket
import sys
import threading
from pathlib import Path

HERE = Path(__file__).resolve().parent
TRACE_FILTER = HERE / "trace_filter.toml"

FRAMEWORKS = ("flask", "django", "fastapi")


def _load_app_module(framework: str):
    """Import ``<framework>/app.py`` by path, under a stable module name.

    Imported by path rather than by package so the demo apps need no
    ``__init__.py`` and no ``sys.path`` surgery — and so the module's
    ``__file__`` is the path the trace filter matches.
    """
    app_path = HERE / framework / "app.py"
    if not app_path.is_file():
        raise SystemExit(f"no demo app at {app_path}")
    spec = importlib.util.spec_from_file_location(f"codetracer_demo_{framework}", app_path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot import {app_path}")
    module = importlib.util.module_from_spec(spec)
    # Registered before execution so decorators referring to the module (and
    # Django's ROOT_URLCONF, which resolves by module name) find it.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _announce(port: int) -> None:
    print(f"READY {port}", flush=True)


def _build_wsgi_server(app, host: str, port: int, threaded: bool = False):
    """Bind a ``wsgiref`` server, before the recording starts.

    Binding early means the listening socket exists before ``READY`` is printed,
    so a client that connects immediately is queued in the backlog and served
    (and therefore recorded) once the serve loop starts.

    Single-threaded by default: requests are handled one at a time, so a
    sequential client sees strictly disjoint span step ranges and the demo's step
    ranges mean what they look like.  ``threaded`` switches to a thread-per-request
    server — how Flask and Django are actually deployed, and a configuration the
    recorder cannot trace today (see
    ``test_threaded_wsgi_requests_land_in_span_stream``).
    """
    from socketserver import ThreadingMixIn
    from wsgiref.simple_server import WSGIRequestHandler, WSGIServer, make_server

    class QuietHandler(WSGIRequestHandler):
        def log_message(self, format, *args):  # noqa: A002 - stdlib signature
            pass

    class ThreadingWSGIServer(ThreadingMixIn, WSGIServer):
        daemon_threads = True

    server_class = ThreadingWSGIServer if threaded else WSGIServer
    httpd = make_server(host, port, app, server_class=server_class, handler_class=QuietHandler)
    # Bounded so the serve loop below notices `stop` promptly without a second
    # thread to wake it.
    httpd.timeout = 0.2
    return httpd


def _serve_wsgi(httpd, stop: threading.Event) -> None:
    """Serve on the MAIN thread until ``stop`` is set.

    Deliberately not ``serve_forever`` on a helper thread: that needs a thread
    created *while tracing is active*, and the recorder cannot reliably trace a
    program that starts threads (the same defect
    ``test_threaded_wsgi_requests_land_in_span_stream`` documents — it hung this
    harness at ``Thread.start()`` before serving a single request).
    ``handle_request`` with a timeout gives the same behaviour with no threads
    at all: one request at a time, and the stop flag checked between requests.
    """
    try:
        while not stop.is_set():
            httpd.handle_request()
    finally:
        httpd.server_close()


def _build_asgi_server(app, host: str, port: int):
    """Construct — and fully LOAD — a uvicorn server, before recording starts.

    Returns ``(server, socket)``: the socket is bound here so ``READY`` can be
    printed before the recording starts without losing an early client (the
    backlog holds it) and so no readiness-watching thread is needed.

    ``Server.serve()`` imports the HTTP protocol implementation (``h11``) and the
    event-loop policy lazily, i.e. inside the recording, where it is by far the
    most expensive thing the recorder sees in this process: measured at over 90
    seconds to reach "listening" on a cold bytecode cache, against ~1 second when
    the same imports happen beforehand.  ``config.load()`` does that work here.
    """
    import uvicorn

    config = uvicorn.Config(app, host=host, port=port, log_level="warning", access_log=False)
    config.load()
    server = uvicorn.Server(config)
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind((host, port))
    sock.listen(64)
    # uvicorn installs its own SIGTERM/SIGINT handlers, which would race the
    # harness's; neutralising the installer leaves shutdown to the flag the
    # harness sets, so the recording is always stopped after the server.
    server.install_signal_handlers = lambda: None
    return server, sock


def _serve_asgi(server, sock, stop: threading.Event) -> None:
    """Serve a pre-built uvicorn server on the MAIN thread until ``stop`` is set.

    ``sock`` is bound before the recording starts (see ``_build_asgi_server``), so
    uvicorn adopts it instead of binding one itself and no thread is needed to
    watch for readiness — for the same reason ``_serve_wsgi`` avoids one.

    The signal handler sets ``server.should_exit``; uvicorn's own loop polls it
    every 100 ms and returns from ``run``, after which the caller stops the
    recording.
    """
    server.run(sockets=[sock])
    del stop  # the signal handler drives `server.should_exit` directly


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--framework", required=True, choices=FRAMEWORKS)
    parser.add_argument("--trace-dir", required=True)
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument(
        "--no-record",
        action="store_true",
        help="serve without recording (used to check the app itself works)",
    )
    parser.add_argument(
        "--threaded",
        action="store_true",
        help=(
            "WSGI only: serve one thread per request, so span registration and "
            "the recorder's monitoring callbacks run on several threads at once"
        ),
    )
    args = parser.parse_args(argv)

    import codetracer_python_recorder as codetracer

    # A stuck request in a recorded server is the failure mode that costs the
    # most time to understand, so make it self-diagnosing: `kill -USR1 <pid>`
    # dumps every thread's Python stack to stderr.
    faulthandler.register(signal.SIGUSR1, all_threads=True)

    stop = threading.Event()

    trace_dir = Path(args.trace_dir)
    trace_dir.mkdir(parents=True, exist_ok=True)

    # Import the framework and build the app BEFORE the recording starts.
    #
    # Handlers are still recorded: `sys.monitoring` enables its events per tool,
    # not per code object, so code imported earlier is traced from the moment
    # tracing starts.  What this avoids is recording the IMPORT of Flask /
    # Django / FastAPI+pydantic / uvicorn, which is pure startup noise the demo
    # never looks at — and which is expensive even when the trace filter skips
    # every line of it: with the import inside the recording, FastAPI took ~30 s
    # to reach "listening" on an idle machine and blew past a 120-second
    # readiness deadline on a loaded one.
    module = _load_app_module(args.framework)
    app = module.build_app(concurrent=args.threaded)

    # Bind the listening socket and print READY before the recording starts: a
    # client that connects immediately waits in the backlog and is still served —
    # and therefore recorded — once the serve loop begins.
    asgi_server, asgi_socket, httpd = None, None, None
    if args.framework == "fastapi":
        asgi_server, asgi_socket = _build_asgi_server(app, args.host, args.port)
        bound_port = asgi_socket.getsockname()[1]
    else:
        httpd = _build_wsgi_server(app, args.host, args.port, threaded=args.threaded)
        bound_port = httpd.server_port

    def request_stop(*_args):
        stop.set()
        if asgi_server is not None:
            # uvicorn's own loop polls this every 100 ms and returns from `run`.
            asgi_server.should_exit = True

    for sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, request_stop)

    session = None
    if not args.no_record:
        session = codetracer.start(trace_dir, trace_filter=str(TRACE_FILTER))

    _announce(bound_port)

    exit_code = 0
    try:
        if asgi_server is not None:
            _serve_asgi(asgi_server, asgi_socket, stop)
        else:
            _serve_wsgi(httpd, stop)
    except Exception as exc:  # noqa: BLE001 - report and still stop the session
        print(f"serve.py failed: {type(exc).__name__}: {exc}", file=sys.stderr, flush=True)
        exit_code = 1
    finally:
        if session is not None:
            # The container — span stream included — is written here.  Without
            # this the process would exit with the trace still in the writer.
            codetracer.stop(exit_code=exit_code)
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
