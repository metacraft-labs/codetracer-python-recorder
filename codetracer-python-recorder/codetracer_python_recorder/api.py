"""High-level tracing API built on a Rust backend.

This module exposes a minimal interface for starting and stopping
runtime traces. The heavy lifting is delegated to the
`codetracer_python_recorder` Rust extension which will eventually hook
into `runtime_tracing` and `sys.monitoring`.  For now the Rust side only
maintains placeholder state and performs no actual tracing.
"""
from __future__ import annotations

import contextlib
import os
from pathlib import Path
from typing import Iterable, Iterator, Optional

from .codetracer_python_recorder import (
    flush_tracing as _flush_backend,
    is_tracing as _is_tracing_backend,
    start_tracing as _start_backend,
    stop_tracing as _stop_backend,
)

TRACE_BINARY: str = "binary"
TRACE_JSON: str = "json"
DEFAULT_FORMAT: str = TRACE_BINARY

_active_session: Optional["TraceSession"] = None


def _normalize_source_roots(source_roots: Iterable[os.PathLike | str] | None) -> Optional[list[str]]:
    if source_roots is None:
        return None
    return [str(Path(p)) for p in source_roots]


def start(
    path: os.PathLike | str,
    *,
    format: str = DEFAULT_FORMAT,
    capture_values: bool = True,
    source_roots: Iterable[os.PathLike | str] | None = None,
) -> "TraceSession":
    """Start a global trace session.

    Parameters mirror the design document.  The current implementation
    merely records the active state on the Rust side and performs no
    tracing.
    """
    global _active_session
    if _is_tracing_backend():
        raise RuntimeError("tracing already active")

    trace_path = Path(path)
    _start_backend(str(trace_path), format, capture_values, _normalize_source_roots(source_roots))
    session = TraceSession(path=trace_path, format=format)
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
    """Flush buffered trace data.

    With the current placeholder implementation this is a no-op but the
    function is provided to match the planned public API.
    """
    if _is_tracing_backend():
        _flush_backend()


@contextlib.contextmanager
def trace(
    path: os.PathLike | str,
    *,
    format: str = DEFAULT_FORMAT,
    capture_values: bool = True,
    source_roots: Iterable[os.PathLike | str] | None = None,
) -> Iterator["TraceSession"]:
    """Context manager helper for scoped tracing."""
    session = start(
        path,
        format=format,
        capture_values=capture_values,
        source_roots=source_roots,
    )
    try:
        yield session
    finally:
        session.stop()


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


def _auto_start_from_env() -> None:
    path = os.getenv("CODETRACER_TRACE")
    if not path:
        return
    fmt = os.getenv("CODETRACER_FORMAT", DEFAULT_FORMAT)
    capture_env = os.getenv("CODETRACER_CAPTURE_VALUES")
    capture = True
    if capture_env is not None:
        capture = capture_env.lower() not in {"0", "false", "no"}
    start(path, format=fmt, capture_values=capture)


_auto_start_from_env()

__all__ = [
    "TraceSession",
    "DEFAULT_FORMAT",
    "TRACE_BINARY",
    "TRACE_JSON",
    "start",
    "stop",
    "is_tracing",
    "trace",
    "flush",
]
