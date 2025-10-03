"""High-level tracing helpers with structured error propagation.

Expose the core session helpers (:func:`start`, :func:`stop`,
:func:`trace`, etc.). These wrappers bubble up :class:`RecorderError`
instances from the Rust layer so callers see stable ``ERR_*`` codes.
"""
from __future__ import annotations

from typing import Iterable

from .formats import DEFAULT_FORMAT, TRACE_BINARY, TRACE_JSON
from .session import TraceSession, flush, is_tracing, start, stop, trace

__all__: Iterable[str] = (
    "TraceSession",
    "DEFAULT_FORMAT",
    "TRACE_BINARY",
    "TRACE_JSON",
    "start",
    "stop",
    "is_tracing",
    "trace",
    "flush",
)
