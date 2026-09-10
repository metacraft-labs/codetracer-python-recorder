"""Correlation markers — declare that a value crossed a boundary.

A *correlation marker* records that some value left this program here, or
arrived here from somewhere else.  Two markers that share a ``boundary``
with opposing directions form a pair, and that pair is what lets CodeTracer
walk an origin chain out of one recording and into another — from the row a
consumer processed back to the line that produced it in the producer's
recording.

Spec: ``codetracer-specs/GUI/Debugging-Features/Correlation-Markers.md``
§2.4 "Programmatic API" owns the spelling used here;
``codetracer-specs/Testing/CTFS-Correlation-Marker-Contract.md`` §§10–11b
owns the mechanism.

Typical use::

    import codetracer_python_recorder as ct

    ct.mark_correlation_send("order-processing", key=msg.id, show=msg.body,
                             desc="Outbound order")
    ct.mark_correlation_recv("order-processing", key=envelope.id,
                             show=envelope.body)

Three things about this module are contract rather than style:

* **Nothing here builds a marker payload.**  The shared CTFS writer library
  does, once, for all ~20 recorders — a recorder whose field names drifted
  would write markers that are *unreadable* rather than degraded, and
  nothing would report an error.  This module renders values to text and
  forwards.
* **Values are stringified HERE, before the recorder is entered.**  ``key``
  and ``show`` accept any object and are passed through :func:`str`.  That
  conversion can run arbitrary ``__str__`` code, which must never happen
  while the writer lock is held, so it happens in this Python frame where no
  lock exists.
* **A marker mints no step.**  It attaches to the line the call sits on.
  Recording a step would insert an event that no user code executed and
  shift every later step index — the indices spans' ``start_step`` /
  ``end_step`` and every other step-addressed coordinate are measured in.

Every function is a silent no-op when no recording is active:
:func:`ensure_marker_id` returns ``None`` and the rest return ``False``.
That is the point — a library may declare its boundaries unconditionally and
still be importable in a process nobody is recording.  It is *not* an error
path, and it is the only case in which ``False`` is returned: a marker that
an active recorder refuses raises.
"""

from __future__ import annotations

from typing import Any, Iterable

from .codetracer_python_recorder import (
    ensure_marker_id as _ensure_marker_id,
    mark_correlation as _mark_correlation,
    mark_correlation_by_id as _mark_correlation_by_id,
    mark_span_coverage as _mark_span_coverage,
)

#: Wire values of a marker's ``direction``.  Two markers pair only when they
#: share a boundary and carry opposing directions, so these are wire
#: constants and not display labels.
DIRECTION_SEND = "send"
DIRECTION_RECV = "recv"


def _render(value: Any) -> str | None:
    """Render *value* to the UTF-8 text a marker field carries.

    ``None`` propagates as ``None`` so the Rust layer can pass the shared
    library's "use your default" sentinel instead of the literal ``"None"``.
    Any other object goes through :func:`str`, and it happens *here* — in
    Python, holding no lock — because ``__str__`` is user code and user code
    must never run inside the recorder's writer lock.
    """
    if value is None:
        return None
    if isinstance(value, str):
        return value
    return str(value)


def ensure_marker_id(label: str) -> int | None:
    """Intern *label* as a boundary and return its marker id.

    THE PRIMARY OPERATION.  Hoist it out of a hot path — once per boundary,
    not once per crossing — and pass the result to
    :func:`mark_correlation_send_by_id` / :func:`mark_correlation_recv_by_id`
    so the per-crossing call does no string lookup::

        ORDERS = ct.ensure_marker_id("order-processing")
        for msg in queue:
            ct.mark_correlation_recv_by_id(ORDERS, "order-processing", key=msg.id)

    Returns ``None`` when no recording is active, which callers must
    distinguish from marker id ``0``.
    """
    return _ensure_marker_id(label)


def mark_correlation_send(
    boundary: str,
    key: Any,
    show: Any = None,
    desc: str | None = None,
    *,
    key_text: str | None = None,
    show_text: str | None = None,
) -> bool:
    """Declare that *key* left this program across *boundary*.

    ``key`` is the match value — the thing the receiving side will see.
    ``show`` is an optional payload displayed alongside it, and ``desc`` an
    optional human description.  Both ``key`` and ``show`` may be any object;
    they are stringified here.

    ``key_text`` / ``show_text`` are the NAMES the two values were read
    under.  ``show_text`` is load-bearing rather than cosmetic: a
    cross-process origin chain resumes its walk on that name in the sending
    recording, so a marker that drops it is visible with its history
    unreachable.  Omit them to take the library's defaults.

    Returns ``True`` when recorded, ``False`` when no recording is active.
    Raises when a recording IS active and the marker cannot be stored.
    """
    return _mark_correlation(
        DIRECTION_SEND,
        boundary,
        _render(key) or "",
        _render(show),
        desc,
        key_text,
        show_text,
    )


def mark_correlation_recv(
    boundary: str,
    key: Any,
    show: Any = None,
    desc: str | None = None,
    *,
    key_text: str | None = None,
    show_text: str | None = None,
) -> bool:
    """Declare that *key* arrived in this program across *boundary*.

    The receiving half of :func:`mark_correlation_send`; see it for the
    argument contract.
    """
    return _mark_correlation(
        DIRECTION_RECV,
        boundary,
        _render(key) or "",
        _render(show),
        desc,
        key_text,
        show_text,
    )


def mark_correlation_send_by_id(
    marker_id: int,
    boundary: str,
    key: Any,
    show: Any = None,
    desc: str | None = None,
    *,
    key_text: str | None = None,
    show_text: str | None = None,
) -> bool:
    """Hot-path form of :func:`mark_correlation_send`.

    *marker_id* comes from :func:`ensure_marker_id`.  *boundary* is still
    required because the recorded payload carries the boundary as text for
    the debugger while the index keys on the id — and a caller that has
    already interned holds the label anyway, so it costs nothing.
    """
    return _mark_correlation_by_id(
        marker_id,
        boundary,
        DIRECTION_SEND,
        _render(key) or "",
        _render(show),
        desc,
        key_text,
        show_text,
    )


def mark_correlation_recv_by_id(
    marker_id: int,
    boundary: str,
    key: Any,
    show: Any = None,
    desc: str | None = None,
    *,
    key_text: str | None = None,
    show_text: str | None = None,
) -> bool:
    """Hot-path form of :func:`mark_correlation_recv`.

    See :func:`mark_correlation_send_by_id`.
    """
    return _mark_correlation_by_id(
        marker_id,
        boundary,
        DIRECTION_RECV,
        _render(key) or "",
        _render(show),
        desc,
        key_text,
        show_text,
    )


def mark_span_coverage(
    trace_id: str,
    span_id: str,
    wall_time_unix_ns: int,
    monotonic_time_ns: int,
) -> bool:
    """Declare that this recording covers a distributed-trace span.

    *trace_id* and *span_id* are the hex forms every OpenTelemetry API hands
    out (``format_trace_id`` / ``format_span_id``): 32 hex characters and 16,
    either case.  The hex is decoded by the shared library so there is one
    implementation of it rather than one per recorder — the correlation index
    keys on the WIRE bytes, and an index keyed on a hex rendering would be
    present, correct-looking, permanently unqueryable, and silent.

    This is what lets an observability consumer holding an OTel
    ``(trace_id, span_id)`` decide whether this recording covers that span
    with one index lookup, rather than by decoding the whole event stream.

    Unlike the ``mark_correlation_*`` calls it mints no marker payload and no
    IO event: a span-coverage marker has no send/recv sense and no pairing
    domain, so forcing it into one would make the pairing index try to pair
    spans with each other.

    Returns ``True`` when recorded, ``False`` when no recording is active.
    """
    return _mark_span_coverage(
        trace_id,
        span_id,
        wall_time_unix_ns,
        monotonic_time_ns,
    )


__all__: Iterable[str] = (
    "DIRECTION_SEND",
    "DIRECTION_RECV",
    "ensure_marker_id",
    "mark_correlation_send",
    "mark_correlation_recv",
    "mark_correlation_send_by_id",
    "mark_correlation_recv_by_id",
    "mark_span_coverage",
)
