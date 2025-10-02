"""High-level tracing API built on a Rust backend."""
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
