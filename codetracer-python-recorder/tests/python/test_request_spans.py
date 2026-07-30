"""RS-M5 — web requests land in the container's span stream.

## Design

Three integration tests, one per supported framework, each of which:

1. starts a **real server process** (`test-programs/web/serve.py`) running a
   **real framework app** (`test-programs/web/{flask,django,fastapi}/app.py`)
   **under the real recorder** — `codetracer.start()` in that process, no
   in-process monkey-patching and no test client;
2. issues **real HTTP requests** over a loopback TCP socket with
   `urllib.request`;
3. stops the server with `SIGTERM`, which stops the recording and writes the
   `.ct` container; and
4. decodes the container's span stream through the **canonical Nim decoder**
   (`codetracer_python_recorder.spans.read_span_stream`, which calls
   `initSpanStreamReader` — the same reader `ct print -f http` uses) and asserts
   on the spans.

**No mocks of any kind appear in this file** (workspace policy requires
justifying every one): there is no fake server, no fake app, no fake container,
no second span decoder. The only test-owned code is the request schedule and the
expectations.

The strongest property asserted here is the one that separates an inline-bound
span from the `codetracer_spans.jsonl` sidecar it replaces: a span's
`[start_step, end_step]` is a coordinate **inside this container**, checked
against the container's own step count, and each request's range covers its own
handler rather than a shared line.

## Scenarios

* `test_flask_requests_land_in_span_stream` — five sequential requests including
  a 500 raised by the handler. Asserts five settled `web-request` spans with the
  right method / URL / status / duration, `http.route` from Werkzeug's matched
  rule, `error.message` on the 500, and step ranges that are inside the
  container, ordered and **disjoint** (the server handles one request at a
  time, so nothing else may appear in a request's range).
* `test_fastapi_async_requests_land_in_span_stream` — three slow async handlers
  issued concurrently plus two fast ones. Asserts the spans **overlap** in step
  space (which is what concurrency on one event loop means for a single step
  timeline), that each still carries its own correct range, that the overlap is
  *declared* (`concurrent_with_siblings` set, `contiguous_on_one_thread` clear),
  and that the in-flight (open) records interleave — i.e. request N+1 opened
  before request N settled, proving the requests really were simultaneous.
* `test_django_requests_land_in_span_stream` — five requests through Django,
  asserting `http.route` carries the URLconf pattern (`api/users/<int:user_id>`)
  and not the concrete URL.

Slow paths sleep 50-60 ms so a non-zero `http.duration_ms` is asserted rather
than assumed; the fast paths are only asserted to be well-formed, since a
sub-millisecond request legitimately rounds to 0 ms.
"""

from __future__ import annotations

import sys
import importlib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Dict, List

import pytest

from codetracer_python_recorder.spans import (
    SPAN_STATUS_ERROR,
    SPAN_STATUS_OK,
    SPAN_TYPE_WEB_REQUEST,
    read_span_stream,
    trace_step_count,
)

# ``test_request_spans.py`` → ``python/`` → ``tests/`` →
# ``codetracer-python-recorder/`` (inner) → repo root.
REPO_ROOT = Path(__file__).resolve().parents[3]
WEB_PROGRAMS = REPO_ROOT / "test-programs" / "web"

# The server harness lives with the test programs because the demo recipe
# (`just demo-request-panel-python`) drives the very same one; duplicating it here
# would let the tested path and the demonstrated path drift apart.
sys.path.insert(0, str(WEB_PROGRAMS))
from session_driver import ServerUnderRecorder  # noqa: E402


def _require(module: str, framework: str):
    """Import a framework, FAILING (never skipping) when it is missing.

    These are required RS-M5 tests, so a missing dependency must be a red test
    and not a quiet skip — `just py-test` installs the `web` dependency group
    precisely so they run.
    """
    try:
        return importlib.import_module(module)
    except ImportError as err:  # pragma: no cover - environment error path
        raise AssertionError(
            f"{framework} is not importable ({err}), so this required RS-M5 test "
            "cannot run.  Install the 'web' dependency group: run `just py-test`, "
            "or `uv run --group dev --group test --group web pytest ...`."
        ) from err


def _web_spans(container: Path) -> List[dict]:
    """The settled ``web-request`` spans of a container, in span-id order."""
    spans = read_span_stream(str(container), settled=True)
    return [span for span in spans if span["span_type"] == SPAN_TYPE_WEB_REQUEST]


def _meta(span: dict) -> Dict[str, str]:
    return {key: value for key, value in span["metadata"]}


def _assert_bound_inside_container(spans: List[dict], container: Path) -> None:
    """Every span's step range must be a real coordinate in this container.

    This is the assertion that distinguishes RS-M5 from the sidecar it replaces:
    a sidecar row had HTTP metadata and no way back into the recording.  The step
    count comes from the container's own exec stream via the canonical reader.
    """
    step_count = trace_step_count(str(container))
    assert step_count > 0, "the recording captured no steps at all"
    for span in spans:
        label = span["label"]
        assert span["start_step"] < step_count, (
            f"{label}: start_step {span['start_step']} is outside the container's "
            f"{step_count} steps"
        )
        assert span["end_step"] < step_count, (
            f"{label}: end_step {span['end_step']} is outside the container's {step_count} steps"
        )
        assert span["end_step"] >= span["start_step"], (
            f"{label}: end_step {span['end_step']} precedes start_step {span['start_step']}"
        )
        # A handler that ran must have executed at least the steps of its own
        # body, so an empty range would mean the binding points at nothing.
        assert span["end_step"] > span["start_step"], (
            f"{label}: step range is empty, so a double-click would seek nowhere "
            f"({span['start_step']}..{span['end_step']})"
        )


def _assert_common_shape(span: dict) -> None:
    """Properties every web-request span must have regardless of framework."""
    label = span["label"]
    assert span["span_type"] == SPAN_TYPE_WEB_REQUEST
    assert not span["is_open"], f"{label}: settled reader returned an open record"
    assert not span["is_external"], f"{label}: an inline span must not be external"
    assert span["shares_timeline"], f"{label}: request shares the process timeline"
    assert span["start_wall_ns"] > 0, f"{label}: no wall-clock start"
    assert span["end_wall_ns"] >= span["start_wall_ns"], f"{label}: end precedes start"
    assert span["thread_id"] > 0, f"{label}: no thread id recorded"
    metadata = _meta(span)
    for required in ("http.method", "http.url", "http.status_code", "http.duration_ms"):
        assert required in metadata, f"{label}: required metadata key {required} missing"


# ---------------------------------------------------------------------------
# flask_requests_land_in_span_stream
# ---------------------------------------------------------------------------


def test_flask_requests_land_in_span_stream(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _require("flask", "Flask")
    trace_dir = tmp_path / "flask-session"

    # RS-M12 removed the sidecar writer.  The opt-in that used to switch it
    # back on is set ON PURPOSE and inherited by the recorded server, so the
    # "no sidecar" assertion below proves the write path is GONE rather than
    # merely switched off by default.
    sidecar_probe = tmp_path / "opt-in-sidecar.jsonl"
    monkeypatch.setenv("CODETRACER_SPAN_MANIFEST", str(sidecar_probe))

    expected = [
        ("GET", "/api/users", 200),
        ("POST", "/api/users", 201),
        ("GET", "/api/users/2", 200),
        ("GET", "/api/boom", 500),
        ("GET", "/api/reports/slow", 200),
    ]

    with ServerUnderRecorder("flask", trace_dir) as server:
        statuses = [
            server.request("/api/users")[0],
            server.request("/api/users", method="POST", body=b'{"name":"Carol"}')[0],
            server.request("/api/users/2")[0],
            server.request("/api/boom")[0],
            server.request("/api/reports/slow")[0],
        ]
    assert statuses == [status for _, _, status in expected], (
        "the server did not answer as expected, so the span assertions below "
        f"would be meaningless: {statuses}"
    )

    container = server.container()
    spans = _web_spans(container)
    assert len(spans) == len(expected), (
        f"expected {len(expected)} web-request spans, got {len(spans)}: "
        f"{[span['label'] for span in spans]}"
    )

    for span, (method, url, status) in zip(spans, expected):
        _assert_common_shape(span)
        metadata = _meta(span)
        assert metadata["http.method"] == method
        assert metadata["http.url"] == url
        assert metadata["http.status_code"] == str(status)
        assert span["label"] == f"{method} {url}"
        assert metadata["framework"] == "flask"
        assert metadata["http.remote_addr"] == "127.0.0.1"
        # Flask matched a rule for every one of these URLs, so the optional
        # route key must be there — and must be the PATTERN, not the URL.
        assert "http.route" in metadata, f"{url}: http.route missing"
        assert int(metadata["http.duration_ms"]) >= 0
        expected_status = SPAN_STATUS_ERROR if status >= 400 else SPAN_STATUS_OK
        assert span["status"] == expected_status, (
            f"{url}: HTTP {status} should map to span status {expected_status}"
        )
        # Sequential WSGI serving: no other request's steps can interleave.
        assert span["contiguous_on_one_thread"], f"{url}: expected a contiguous span"
        assert not span["concurrent_with_siblings"]

    # The parameterised route keeps its pattern rather than the concrete id.
    assert _meta(spans[2])["http.route"] == "/api/users/<int:user_id>"

    # The 500 came from a raising handler, so it must carry a diagnosis.
    boom = spans[3]
    assert _meta(boom)["error.message"] == "handler raised on purpose"

    # The slow handler sleeps 50 ms; a duration that did not measure the handler
    # would come back as 0.
    slow = spans[4]
    assert int(_meta(slow)["http.duration_ms"]) >= 40, (
        f"the 50 ms handler reported {_meta(slow)['http.duration_ms']} ms"
    )

    _assert_bound_inside_container(spans, container)

    # One request at a time, so the ranges must be strictly ordered and
    # disjoint: request N+1 starts after request N ended.
    for previous, current in zip(spans, spans[1:]):
        assert current["start_step"] > previous["end_step"], (
            f"{current['label']} starts at step {current['start_step']} but "
            f"{previous['label']} only ended at {previous['end_step']} — "
            "sequentially served requests must not share steps"
        )

    # Span ids are 1-based and dense for a session whose only spans are requests.
    assert [span["span_id"] for span in spans] == list(range(1, len(expected) + 1))

    # RS-M5's definition of done: the recorded path involves no sidecar file.
    # RS-M12 went further and removed the writer, so neither the trace
    # directory nor the explicitly requested opt-in path may hold one.
    assert not list(trace_dir.glob("*.jsonl")), "a sidecar manifest was written"
    assert not sidecar_probe.exists(), (
        "CODETRACER_SPAN_MANIFEST must no longer produce a JSONL sidecar"
    )


# ---------------------------------------------------------------------------
# fastapi_async_requests_land_in_span_stream
# ---------------------------------------------------------------------------


def test_fastapi_async_requests_land_in_span_stream(tmp_path: Path) -> None:
    _require("fastapi", "FastAPI")
    _require("uvicorn", "uvicorn")
    trace_dir = tmp_path / "fastapi-session"

    with ServerUnderRecorder("fastapi", trace_dir) as server:
        # Three slow handlers in flight at once: each awaits ~120 ms, so all
        # three are suspended in the event loop simultaneously and their steps
        # interleave in the single recorded timeline.
        with ThreadPoolExecutor(max_workers=3) as pool:
            futures = [
                pool.submit(server.request, f"/api/slow/{token}?ms=120")
                for token in ("alpha", "beta", "gamma")
            ]
            concurrent_statuses = [future.result()[0] for future in futures]
        sequential_statuses = [
            server.request("/api/users")[0],
            server.request("/api/users/99")[0],
        ]

    assert concurrent_statuses == [200, 200, 200]
    assert sequential_statuses == [200, 404]

    container = server.container()
    spans = _web_spans(container)
    assert len(spans) == 5, (
        f"expected 5 web-request spans, got {len(spans)}: {[span['label'] for span in spans]}"
    )

    for span in spans:
        _assert_common_shape(span)
        metadata = _meta(span)
        assert metadata["framework"] == "fastapi"
        # ASGI handlers interleave by construction, so every span declares that
        # its range may overlap a sibling's and is not a contiguous call trace.
        assert span["concurrent_with_siblings"], f"{span['label']}: overlap not declared"
        assert not span["contiguous_on_one_thread"], (
            f"{span['label']}: an interleaved async span must not claim to be "
            "contiguous on one thread"
        )

    slow_spans = [span for span in spans if "/api/slow/" in _meta(span)["http.url"]]
    assert len(slow_spans) == 3
    for span in slow_spans:
        metadata = _meta(span)
        assert metadata["http.status_code"] == "200"
        assert span["status"] == SPAN_STATUS_OK
        assert int(metadata["http.duration_ms"]) >= 100, (
            f"{metadata['http.url']} awaited 120 ms but reported {metadata['http.duration_ms']} ms"
        )
        # FastAPI's route pattern, not the concrete token.
        assert metadata["http.route"] == "/api/slow/{token}"
        assert "?ms=120" in metadata["http.url"], "the query string is part of the recorded URL"

    # The 404 is a real request with a real span, and its status maps to error.
    missing = [span for span in spans if _meta(span)["http.status_code"] == "404"]
    assert len(missing) == 1
    assert missing[0]["status"] == SPAN_STATUS_ERROR

    _assert_bound_inside_container(spans, container)

    # --- the concurrency claim ------------------------------------------
    #
    # Overlapping ranges are the observable consequence of interleaved async
    # handlers sharing one step timeline.  At least one pair of the three
    # simultaneous requests must overlap; if the server had serialised them the
    # ranges would be disjoint and this would fail.
    ranges = sorted((span["start_step"], span["end_step"], span["label"]) for span in slow_spans)
    overlaps = [(a[2], b[2]) for a, b in zip(ranges, ranges[1:]) if b[0] <= a[1]]
    assert overlaps, (
        "the three concurrent requests produced disjoint step ranges, so they "
        f"were not in flight together: {ranges}"
    )

    # Each still owns a distinct range — overlapping must not mean identical,
    # which would indicate the spans lost their per-request state.
    assert len({(span["start_step"], span["end_step"]) for span in slow_spans}) == 3, (
        f"concurrent spans collapsed onto the same step range: {ranges}"
    )

    # And the in-flight records prove the overlap independently of step ranges:
    # in APPEND order, an open record appears after a previous request's open
    # record but before its completion.
    raw = [
        span
        for span in read_span_stream(str(container), settled=False)
        if span["span_type"] == SPAN_TYPE_WEB_REQUEST
    ]
    open_ids: List[int] = []
    interleaved = False
    for record in raw:
        if record["is_open"]:
            if open_ids:
                interleaved = True
            open_ids.append(record["span_id"])
        else:
            if record["span_id"] in open_ids:
                open_ids.remove(record["span_id"])
    assert interleaved, (
        "no request was open while another was already open, so nothing was "
        f"concurrent: {[(r['span_id'], r['is_open']) for r in raw]}"
    )


# ---------------------------------------------------------------------------
# django_requests_land_in_span_stream
# ---------------------------------------------------------------------------


def test_django_requests_land_in_span_stream(tmp_path: Path) -> None:
    _require("django", "Django")
    trace_dir = tmp_path / "django-session"

    expected = [
        ("GET", "/api/users", 200, "api/users"),
        ("POST", "/api/users/new", 201, "api/users/new"),
        ("GET", "/api/users/2", 200, "api/users/<int:user_id>"),
        ("GET", "/api/users/999", 404, "api/users/<int:user_id>"),
        ("GET", "/api/reports/slow", 200, "api/reports/slow"),
    ]

    with ServerUnderRecorder("django", trace_dir) as server:
        statuses = [
            server.request("/api/users")[0],
            server.request("/api/users/new", method="POST", body=b'{"name":"Dave"}')[0],
            server.request("/api/users/2")[0],
            server.request("/api/users/999")[0],
            server.request("/api/reports/slow")[0],
        ]
    assert statuses == [status for _, _, status, _ in expected], statuses

    container = server.container()
    spans = _web_spans(container)
    assert len(spans) == len(expected), (
        f"expected {len(expected)} web-request spans, got {len(spans)}: "
        f"{[span['label'] for span in spans]}"
    )

    for span, (method, url, status, route) in zip(spans, expected):
        _assert_common_shape(span)
        metadata = _meta(span)
        assert metadata["http.method"] == method
        assert metadata["http.url"] == url
        assert metadata["http.status_code"] == str(status)
        assert metadata["framework"] == "django"
        # The milestone's specific Django requirement: http.route is populated,
        # and carries the URLconf pattern rather than the concrete path.
        assert metadata["http.route"] == route, (
            f"{url}: expected route {route!r}, got {metadata.get('http.route')!r}"
        )
        expected_status = SPAN_STATUS_ERROR if status >= 400 else SPAN_STATUS_OK
        assert span["status"] == expected_status

    slow = spans[4]
    assert int(_meta(slow)["http.duration_ms"]) >= 40, (
        f"the 50 ms view reported {_meta(slow)['http.duration_ms']} ms"
    )

    _assert_bound_inside_container(spans, container)
    for previous, current in zip(spans, spans[1:]):
        assert current["start_step"] > previous["end_step"], (
            f"{current['label']} overlaps {previous['label']} although Django was "
            "served one request at a time"
        )


# ---------------------------------------------------------------------------
# The middleware without a recording, and the sidecar's retirement
# ---------------------------------------------------------------------------


def test_middleware_without_a_recording_writes_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A wrapped app that is not being recorded must still work.

    The middleware is installed at import time in a real deployment, so it has
    to be harmless when no session is active: no span, no file, no exception.

    RS-M12 removed the sidecar writer, so the opt-in is switched ON here and
    the assertion is that nothing appears anyway.  Asserting on an UNSET
    variable would only have tested today's default.
    """
    from codetracer_python_recorder.middleware.wsgi import CodeTracerWSGIMiddleware

    manifest = tmp_path / "spans.jsonl"
    monkeypatch.setenv("CODETRACER_SPAN_MANIFEST", str(manifest))

    def app(environ, start_response):
        start_response("200 OK", [("Content-Type", "text/plain"), ("Content-Length", "2")])
        return [b"ok"]

    wrapped = CodeTracerWSGIMiddleware(app)
    body = wrapped({"REQUEST_METHOD": "GET", "PATH_INFO": "/health"}, lambda *_: None)
    assert body == [b"ok"]
    assert not manifest.exists(), (
        "CODETRACER_SPAN_MANIFEST must no longer produce a sidecar"
    )

    # The parameter went with the writer.  It was the SECOND POSITIONAL
    # argument, so a caller still passing it gets a TypeError rather than a
    # silently ignored path.
    with pytest.raises(TypeError):
        CodeTracerWSGIMiddleware(app, str(manifest))


# ---------------------------------------------------------------------------
# Threaded WSGI serving — a pre-existing recorder defect, kept executable
# ---------------------------------------------------------------------------


@pytest.mark.xfail(
    strict=True,
    reason=(
        "PRE-EXISTING RECORDER DEFECT, not a span bug: the recorder cannot trace "
        "a multi-threaded program while several threads are active.  Every "
        "sys.monitoring callback in src/monitoring/callbacks.rs takes the GLOBAL "
        "tracer mutex WHILE HOLDING THE GIL, so as soon as one callback is "
        "running and another thread hits an event, the second thread blocks on "
        "the mutex without releasing the GIL and the first can never finish.  "
        "Reproduced with the span middleware entirely removed (a plain Flask app "
        "on a thread-per-request wsgiref server, recorder on, four concurrent "
        "requests: all four time out), so fixing it means restructuring the "
        "recorder's callback locking — out of scope for RS-M5.  When that lands, "
        "this test should pass and strict xfail will say so by failing."
    ),
)
def test_threaded_wsgi_requests_land_in_span_stream(tmp_path: Path) -> None:
    """Flask on a thread-per-request server, four requests at once.

    This is how Flask and Django are actually deployed, so it is worth having an
    executable statement of what happens today.  The span entry points are ready
    for it — they take the tracer lock only while holding the GIL and release the
    GIL while waiting for it, which is why a SINGLE request on a threaded server
    works — but the recorder's own callbacks are not, hence the xfail above.

    The waits are deliberately short: this test is expected to fail, and it
    should do so in seconds rather than tie up CI for minutes.
    """
    _require("flask", "Flask")
    trace_dir = tmp_path / "flask-threaded-session"
    slow_urls = [f"/api/reports/slow?worker={i}" for i in range(4)]

    with ServerUnderRecorder(
        "flask",
        trace_dir,
        threaded=True,
        request_timeout=10.0,
        stop_timeout=20.0,
    ) as server:
        with ThreadPoolExecutor(max_workers=len(slow_urls)) as pool:
            futures = [pool.submit(server.request, url) for url in slow_urls]
            statuses = [future.result()[0] for future in futures]

    assert statuses == [200] * len(slow_urls)

    container = server.container()
    spans = _web_spans(container)
    assert len(spans) == len(slow_urls), (
        f"expected {len(slow_urls)} spans, got {len(spans)}: {[span['label'] for span in spans]}"
    )
    for span in spans:
        _assert_common_shape(span)
        metadata = _meta(span)
        assert metadata["http.status_code"] == "200"
        assert metadata["http.route"] == "/api/reports/slow"
        assert span["concurrent_with_siblings"], (
            "a threaded server's spans must declare that they may overlap"
        )
    _assert_bound_inside_container(spans, container)

    # Each request ran on its own worker thread, and the span says which — the
    # per-request state cannot have been shared through a thread-local.
    thread_ids = {span["thread_id"] for span in spans}
    assert len(thread_ids) > 1, (
        f"all {len(spans)} concurrent requests reported the same thread id "
        f"{thread_ids}, so the server did not actually thread them"
    )
    # The queries distinguish the requests, so every span kept its own URL.
    assert sorted(_meta(span)["http.url"] for span in spans) == sorted(slow_urls)
