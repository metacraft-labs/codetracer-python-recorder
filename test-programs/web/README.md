# Web-framework demo apps (RS-M5)

Three tiny servers, one per supported framework, used by both the recorder's
request-span integration tests and `just demo-request-panel-python`:

| Directory  | Framework | Server surface                                     |
| ---------- | --------- | -------------------------------------------------- |
| `flask/`   | Flask     | WSGI, wrapped with `CodeTracerWSGIMiddleware`       |
| `django/`  | Django    | WSGI, wrapped with `CodeTracerWSGIMiddleware`       |
| `fastapi/` | FastAPI   | ASGI, wrapped with `CodeTracerASGIMiddleware`       |

Each `app.py` exposes:

* `build_<framework>_app()` — the bare app, no recorder involved, and
* `build_app(concurrent=False)` — the middleware-wrapped application object the
  harness serves.

They are deliberately *small and boring*: every route is deterministic, and the
handlers are the only user code in the trace, so a request's span step range is
exactly its handler's steps. The routes cover the status buckets the Request
Panel colours (2xx, 3xx, 4xx, 5xx), a handler that raises, and a slow handler so
a duration is visibly non-zero.

All three are recorded through the shared trace filter `trace_filter.toml` in
this directory, which skips everything except the app module. Read its comments
before changing it: recording a Flask or uvicorn stack line-by-line costs
31_862 steps and ~156 s just to reach "listening", and a filter that
accidentally also matches Flask's *own* `flask/app.py` makes every request row's
double-click land in the framework instead of in the handler. Both mistakes were
made and are documented in the file.

`serve.py` is the shared entry point: it starts a recording, serves until told
to stop (`SIGTERM`), and stops the recording so the container — span stream
included — is written. `session_driver.py` drives it: the integration tests use
its `ServerUnderRecorder` directly, and `just demo-request-panel-python` (plus
the codetracer fixture regeneration) runs it as a script to record the
`DEMO_REQUESTS` schedule. The tests and the demo therefore exercise the same
harness rather than two that can drift apart.
