"""Tracing session management helpers."""
from __future__ import annotations

import contextlib
from pathlib import Path
from typing import Iterator, Optional

from .codetracer_python_recorder import (
    flush_tracing as _flush_backend,
    is_tracing as _is_tracing_backend,
    start_tracing as _start_backend,
    stop_tracing as _stop_backend,
)
from .formats import DEFAULT_FORMAT, normalize_format

_active_session: Optional["TraceSession"] = None


class TraceSession:
    """Handle representing a live tracing session."""

    path: Path
    format: str

    def __init__(self, path: Path, format: str) -> None:
        self.path = path
        self.format = format

    def stop(self) -> None:
        """Stop this trace session."""
        if _active_session is self:
            stop()

    def flush(self) -> None:
        """Flush buffered trace data for this session."""
        flush()

    def __enter__(self) -> "TraceSession":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:  # pragma: no cover - thin wrapper
        self.stop()


def start(
    path: str | Path,
    *,
    format: str = DEFAULT_FORMAT,
    start_on_enter: str | Path | None = None,
) -> TraceSession:
    """Start a new global trace session."""
    global _active_session
    if _is_tracing_backend():
        raise RuntimeError("tracing already active")

    trace_path = Path(path)
    normalized_format = normalize_format(format)

    _start_backend(
        str(trace_path),
        normalized_format,
        str(Path(start_on_enter)) if start_on_enter is not None else None,
    )
    session = TraceSession(path=trace_path, format=normalized_format)
    _active_session = session
    return session


def stop() -> None:
    """Stop the active trace session if one is running."""
    global _active_session
    if not _is_tracing_backend():
        return
    _stop_backend()
    _active_session = None


def is_tracing() -> bool:
    """Return ``True`` when a trace session is active."""
    return _is_tracing_backend()


def flush() -> None:
    """Flush buffered trace data."""
    if _is_tracing_backend():
        _flush_backend()


@contextlib.contextmanager
def trace(
    path: str | Path,
    *,
    format: str = DEFAULT_FORMAT,
) -> Iterator[TraceSession]:
    """Context manager helper for scoped tracing."""
    session = start(path, format=format)
    try:
        yield session
    finally:
        session.stop()


__all__ = (
    "TraceSession",
    "flush",
    "is_tracing",
    "start",
    "stop",
    "trace",
)
