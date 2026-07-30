"""RS-M5 — the request-span lifecycle shared by the WSGI and ASGI middleware.

## Where a span begins and ends

One HTTP request is one span:

* **begin** — before the wrapped application is invoked.  The span id is
  allocated, ``start_step`` is read from the recorder (the index the *next*
  recorded event will take, i.e. the first step of the handler), and an OPEN
  record is appended so a live consumer sees an in-flight row.
* **end** — after the application has produced its response (or raised).  A
  second record with the SAME span id carries the status, the duration, the
  response size and ``end_step`` (the last step recorded during the request).
  Readers apply last-record-wins, so the pair settles into one span.

## How concurrent requests stay distinct

All per-request state lives on the :class:`PendingRequestSpan` object returned by
:meth:`RequestSpanRecorder.begin`, which the caller keeps in a local variable —
a WSGI worker thread's stack frame, or an ASGI coroutine's frame.  There is no
thread-local and no "current span" global, so two requests being served at the
same time cannot see each other's state.  This is what makes the async case work
at all: FastAPI handlers interleave on ONE thread, so a thread-local design
would have collapsed concurrent requests into each other.

Because the recorder writes a single step timeline for the whole process,
concurrently served requests genuinely OVERLAP in step space.  Spans say so
rather than pretending otherwise: an overlapping span is marked
``concurrent_with_siblings`` and is not marked ``contiguous_on_one_thread``
(Trace-Spans.md § 2.4), and it carries the OS thread id it ran on.

## Known limitation: threaded WSGI servers

A thread-per-request server (Flask/Django behind gunicorn threads, or
``wsgiref`` with ``ThreadingMixIn``) does not work **under the recorder** today,
and not because of anything here: every ``sys.monitoring`` callback in the
recorder takes its global tracer lock while holding the GIL, so two threads
executing traced code at once deadlock.  Reproduced with this middleware removed
entirely, and pinned by the strict-xfail
``test_threaded_wsgi_requests_land_in_span_stream``.  Single-threaded serving —
what every demo and integration test here uses — is unaffected, and the span
entry points themselves are already thread-safe (they hold the tracer lock only
while holding the GIL, and release the GIL while waiting for it).

## No sidecar (RS-M12)

Until RS-M5 this middleware wrote request metadata to a
``codetracer_spans.jsonl`` sidecar; RS-M5 moved it into the container's span
stream and kept the sidecar writer one release behind an opt-in
``CODETRACER_SPAN_MANIFEST``.  RS-M12 removed that writer: nothing here opens a
file, and the environment variable is no longer read.  A sidecar row is not
seekable — it names no coordinate in any recording — which is exactly what the
span stream fixed, so there was nothing left for it to carry.  Sessions
recorded before the change are still readable through CodeTracer's db-backend
shim (``src/db-backend/src/request_spans.rs``).
"""

from __future__ import annotations

import threading
import time
from typing import Iterable, List, Optional, Tuple

from ..spans import (
    SPAN_STATUS_ERROR,
    SPAN_STATUS_OK,
    SPAN_STATUS_UNKNOWN,
    SPAN_TYPE_WEB_REQUEST,
    allocate_span_id,
    next_step_index,
    register_span,
)


def _current_thread_id() -> int:
    """The OS thread id of the calling thread, with a portable fallback."""
    native_id = getattr(threading, "get_native_id", None)
    if native_id is not None:
        try:
            return int(native_id())
        except OSError:  # pragma: no cover - platform without native ids
            pass
    return int(threading.get_ident())


def _status_for(status_code: int) -> int:
    """Map an HTTP status to a span status.

    ``>= 400`` is an error — the same mapping the sidecar JSONL used, and the
    one the Request Panel's colouring assumes.
    """
    if status_code <= 0:
        return SPAN_STATUS_UNKNOWN
    return SPAN_STATUS_ERROR if status_code >= 400 else SPAN_STATUS_OK


class PendingRequestSpan:
    """One in-flight request.  Created by :meth:`RequestSpanRecorder.begin`.

    Holding all request state here (rather than in a thread-local) is what keeps
    concurrent requests — threaded WSGI workers *and* interleaved ASGI
    coroutines on a single thread — from overwriting each other.
    """

    __slots__ = (
        "_recorder",
        "span_id",
        "method",
        "url",
        "remote_addr",
        "start_wall_ns",
        "_start_perf_ns",
        "start_step",
        "thread_id",
        "recorded",
        "_settled",
    )

    def __init__(
        self,
        recorder: "RequestSpanRecorder",
        span_id: int,
        method: str,
        url: str,
        remote_addr: str,
        start_step: Optional[int],
    ) -> None:
        self._recorder = recorder
        self.span_id = span_id
        self.method = method
        self.url = url
        self.remote_addr = remote_addr
        self.start_wall_ns = time.time_ns()
        self._start_perf_ns = time.perf_counter_ns()
        # ``None`` means "not being recorded"; step 0 is a real step index, so
        # the two must not be conflated.
        self.start_step = start_step
        # The OS-level thread id, so the value means something outside this
        # process (it is the TID in /proc and in a debugger).  NOT an index into
        # a thread table: the Python recorder does not emit thread lifecycle
        # events yet, so the container has no thread table to index into.  The
        # field's use today is telling concurrent requests apart.
        self.thread_id = _current_thread_id()
        self.recorded = False
        self._settled = False

    @property
    def label(self) -> str:
        return f"{self.method} {self.url}"

    def _metadata(
        self,
        status_code: int,
        duration_ms: int,
        response_size: Optional[int],
        route: Optional[str],
        error_message: Optional[str],
    ) -> List[Tuple[str, str]]:
        """The well-known ``http.*`` keys, in display order.

        Order is part of the wire contract (readers hand metadata back in
        emission order), so this is built as a list and never from a dict
        comprehension over an unordered source.
        """
        metadata: List[Tuple[str, str]] = [
            ("http.method", self.method),
            ("http.url", self.url),
            ("http.status_code", str(status_code)),
            ("http.duration_ms", str(duration_ms)),
        ]
        if route:
            metadata.append(("http.route", route))
        if response_size is not None:
            metadata.append(("http.response_size", str(response_size)))
        if self.remote_addr:
            metadata.append(("http.remote_addr", self.remote_addr))
        if self._recorder.framework:
            metadata.append(("framework", self._recorder.framework))
        if error_message:
            metadata.append(("error.message", error_message))
        return metadata

    def publish_open(self) -> None:
        """Append the OPEN record: the request has started, nothing else known."""
        if self.start_step is None:
            return
        self.recorded = register_span(
            self.span_id,
            SPAN_TYPE_WEB_REQUEST,
            self.label,
            status=SPAN_STATUS_UNKNOWN,
            start_wall_ns=self.start_wall_ns,
            start_step=self.start_step,
            thread_id=self.thread_id,
            is_open=True,
            shares_timeline=True,
            concurrent_with_siblings=self._recorder.concurrent,
            metadata=self._metadata(0, 0, None, None, None),
        )

    def finish(
        self,
        status_code: int,
        *,
        response_size: Optional[int] = None,
        route: Optional[str] = None,
        error_message: Optional[str] = None,
    ) -> None:
        """Append the settled record for this request.

        Idempotent: a middleware that both catches an exception and runs a
        ``finally`` block cannot double-append.
        """
        if self._settled:
            return
        self._settled = True
        duration_ms = (time.perf_counter_ns() - self._start_perf_ns) // 1_000_000
        end_wall_ns = self.start_wall_ns + (time.perf_counter_ns() - self._start_perf_ns)
        metadata = self._metadata(status_code, duration_ms, response_size, route, error_message)

        if self.start_step is not None:
            end_step = next_step_index()
            # ``end_step`` is the LAST step inside the span, so it is one before
            # the next index.  A request during which nothing was recorded (an
            # unfiltered-out handler, a 404 that never enters user code) collapses
            # to the single step it started at rather than wrapping around.
            if end_step is None or end_step <= self.start_step:
                end_step = self.start_step
            else:
                end_step -= 1
            self.recorded = register_span(
                self.span_id,
                SPAN_TYPE_WEB_REQUEST,
                self.label,
                status=_status_for(status_code),
                start_wall_ns=self.start_wall_ns,
                end_wall_ns=end_wall_ns,
                start_step=self.start_step,
                end_step=end_step,
                thread_id=self.thread_id,
                # A request that overlaps its siblings in step space is not
                # contiguous on one thread; saying so is what tells a UI it may
                # not render the range as a continuous call trace.
                contiguous_on_one_thread=not self._recorder.concurrent,
                shares_timeline=True,
                concurrent_with_siblings=self._recorder.concurrent,
                metadata=metadata,
            )


class RequestSpanRecorder:
    """Allocates and publishes one span per HTTP request.

    ``framework`` is recorded as the ``framework`` metadata key (``flask`` /
    ``django`` / ``fastapi``).  ``concurrent`` marks the spans this recorder
    produces as possibly overlapping their siblings — true for ASGI, where
    handlers interleave on one thread, and for threaded WSGI servers.
    ``publish_open`` controls whether an in-flight record is appended at request
    start; on by default because it is what makes a live panel show a request
    before it finishes.
    """

    def __init__(
        self,
        *,
        framework: str = "",
        concurrent: bool = False,
        publish_open: bool = True,
    ) -> None:
        self.framework = framework
        self.concurrent = concurrent
        self.publish_open = publish_open

    def begin(
        self,
        method: str,
        url: str,
        remote_addr: str = "",
    ) -> PendingRequestSpan:
        """Open a span for a request that is about to be handled."""
        pending = PendingRequestSpan(
            self,
            allocate_span_id(),
            method,
            url,
            remote_addr,
            next_step_index(),
        )
        if self.publish_open:
            pending.publish_open()
        return pending


def parse_status_code(status: object) -> int:
    """Extract the numeric status from a WSGI ``"200 OK"`` status line.

    Returns ``0`` for a missing or malformed status — the same "unknown"
    encoding the span record uses — rather than guessing 200.
    """
    if status is None:
        return 0
    try:
        return int(str(status).split(" ")[0])
    except (ValueError, IndexError):
        return 0


__all__: Iterable[str] = (
    "PendingRequestSpan",
    "RequestSpanRecorder",
    "parse_status_code",
)
