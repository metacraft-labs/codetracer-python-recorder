"""RS-M5 — span emission into the recorded ``.ct`` container.

A *span* is a bounded, labeled interval of execution — an HTTP request, a
process, a test.  Since RS-M1 the trace container carries them in its own
``spans.dat`` / ``spans.idx`` stream (spec:
``codetracer-specs/Trace-Files/CTFS-Request-Span-Streams.md``), and since this
milestone the WSGI / ASGI middleware write there instead of appending to a
``codetracer_spans.jsonl`` sidecar.

The difference that matters is *binding*: a sidecar span was a row of HTTP
metadata with no way back into the recording, while a span record names a
``(process, thread, step range)`` coordinate INSIDE the container being
recorded.  That is what lets CodeTracer's Request Panel seek from a request row
to the first step of that request's handler.

This module is the thin Python face of three Rust primitives (see
``src/spans.rs``); the request-shaped logic lives in
:mod:`codetracer_python_recorder.middleware.span_recorder`.

Nothing here raises when no recording is active: :func:`next_step_index`
returns ``None`` and :func:`register_span` returns ``False``, so middleware can
be installed unconditionally in an app that is only sometimes recorded.
"""

from __future__ import annotations

from typing import Iterable, Sequence, Tuple

from .codetracer_python_recorder import (
    read_span_stream_json as _read_span_stream_json,
    register_span as _register_span,
    span_allocate_id as _span_allocate_id,
    span_next_step_index as _span_next_step_index,
    trace_step_count as _trace_step_count,
)

#: Wire values of a span record's ``status`` byte.
SPAN_STATUS_UNKNOWN = 0
SPAN_STATUS_OK = 1
SPAN_STATUS_ERROR = 2

#: ``span_type`` of an HTTP request span.  The Request Panel selects rows by
#: this exact string (via the container's ``spantype.ns`` index), so it is a
#: wire constant and not a display label.
SPAN_TYPE_WEB_REQUEST = "web-request"


def allocate_span_id() -> int:
    """Return the next container-unique, 1-based, monotonic span id.

    Allocated by the recorder rather than per-middleware so a process serving
    both WSGI and ASGI traffic cannot mint colliding ids.
    """
    return _span_allocate_id()


def next_step_index() -> int | None:
    """The step index the next recorded event will occupy, or ``None``.

    ``None`` means no session is active, which callers must distinguish from
    step ``0``.  A span that runs from here to there carries
    ``start_step = next_step_index()`` at entry and
    ``end_step = next_step_index() - 1`` at exit.

    The value comes from the trace WRITER's own step counter, which advances for
    every exec-stream event (steps, column deltas, raise/catch, thread events) —
    it is that counter which defines the step ids a reader walks, so a
    recorder-side count of step registrations would drift from it.
    """
    return _span_next_step_index()


def register_span(
    span_id: int,
    span_type: str,
    label: str,
    *,
    status: int = SPAN_STATUS_UNKNOWN,
    start_wall_ns: int = 0,
    end_wall_ns: int = 0,
    start_step: int = 0,
    end_step: int = 0,
    thread_id: int = 0,
    process_ord: int = 0,
    parent_span_id: int = 0,
    is_open: bool = False,
    contiguous_on_one_thread: bool = False,
    shares_timeline: bool = True,
    concurrent_with_siblings: bool = False,
    metadata: Sequence[Tuple[str, str]] = (),
) -> bool:
    """Append one span record to the active recording's span stream.

    Returns ``True`` when the span was recorded, ``False`` when no session is
    active.  Raises when a session IS active and the span cannot be stored, so
    a recorded run never loses requests silently.

    ``metadata`` is an ordered sequence of ``(key, value)`` pairs, never a
    mapping: metadata order is part of the wire contract and consumers render
    it in emission order.

    Publishing an in-flight interval is two calls with the same ``span_id`` —
    one with ``is_open=True``, then the settled one.  Readers apply
    last-record-wins.
    """
    return _register_span(
        span_id,
        span_type,
        label,
        status=status,
        start_wall_ns=start_wall_ns,
        end_wall_ns=end_wall_ns,
        start_step=start_step,
        end_step=end_step,
        thread_id=thread_id,
        process_ord=process_ord,
        parent_span_id=parent_span_id,
        is_open=is_open,
        contiguous_on_one_thread=contiguous_on_one_thread,
        shares_timeline=shares_timeline,
        concurrent_with_siblings=concurrent_with_siblings,
        metadata=list(metadata),
    )


def read_span_stream(path: str, *, settled: bool = True) -> list[dict]:
    """Decode the span stream of the ``.ct`` container at *path*.

    Goes through the canonical Nim decoder — the same one ``ct print -f http``
    uses — so a caller verifying emitted spans is not reading them back through
    a second decoder that could share a bug with the writer.

    ``settled=True`` applies last-record-wins per ``span_id`` and sorts
    ascending by ``span_id`` (what a panel displays).  ``settled=False`` returns
    every record in append order, open records included.

    Each returned dict uses the spec's wire field names; ``metadata`` is a list
    of ``[key, value]`` pairs, because metadata order is part of the contract.
    """
    import json

    return json.loads(_read_span_stream_json(str(path), settled))


def trace_step_count(path: str) -> int:
    """The number of steps recorded in the ``.ct`` container at *path*.

    Lets a consumer check that a span's ``[start_step, end_step]`` really is a
    coordinate INSIDE that container — the property that distinguishes an
    inline-bound span from the sidecar rows it replaces.
    """
    return _trace_step_count(str(path))


def span_metadata_value(span: dict, key: str, default: str = "") -> str:
    """Look one metadata key up in a span decoded by :func:`read_span_stream`.

    Metadata arrives as an ordered list of pairs (order is a wire guarantee),
    so a lookup is a scan.  Provided here so every consumer scans it the same
    way instead of rebuilding a dict and losing the order.
    """
    for pair in span.get("metadata", ()):
        if pair[0] == key:
            return pair[1]
    return default


__all__: Iterable[str] = (
    "SPAN_STATUS_UNKNOWN",
    "SPAN_STATUS_OK",
    "SPAN_STATUS_ERROR",
    "SPAN_TYPE_WEB_REQUEST",
    "allocate_span_id",
    "next_step_index",
    "register_span",
    "read_span_stream",
    "span_metadata_value",
    "trace_step_count",
)
