"""Tracing session management helpers with policy integration.

These wrappers load policy from env vars, call into the Rust backend,
and surface structured :class:`RecorderError` instances on failure.
"""
from __future__ import annotations

import contextlib
import os
from collections.abc import Sequence
from pathlib import Path
from typing import Iterator, Mapping, Optional

from .codetracer_python_recorder import (
    configure_policy as _configure_policy,
    configure_policy_from_env as _configure_policy_from_env,
    flush_tracing as _flush_backend,
    is_tracing as _is_tracing_backend,
    start_tracing as _start_backend,
    stop_tracing as _stop_backend,
)
from .formats import DEFAULT_FORMAT, SUPPORTED_FORMATS, is_supported, normalize_format

_active_session: Optional["TraceSession"] = None


class TraceSession:
    """Handle representing a live tracing session.

    The object keeps the resolved trace path and format. Use
    :meth:`flush` and :meth:`stop` to interact with the global session.
    """

    path: Path
    format: str

    def __init__(self, path: Path, format: str) -> None:
        self.path = path
        self.format = format

    def stop(self, *, exit_code: int | None = None) -> None:
        """Stop this trace session.

        Parameters
        ----------
        exit_code:
            Optional process exit status to forward to the recorder backend.
            When ``None``, the session shutdown reason remains unspecified.
        """
        if _active_session is self:
            stop(exit_code=exit_code)

    def flush(self) -> None:
        """Flush buffered trace data for this session."""
        flush()

    def __enter__(self) -> "TraceSession":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:  # pragma: no cover - thin wrapper
        # Exit codes are not tracked for context-managed sessions; report unknown.
        self.stop()


def start(
    path: str | Path,
    *,
    format: str = DEFAULT_FORMAT,
    start_on_enter: str | Path | None = None,
    trace_filter: str | os.PathLike[str] | Sequence[str | os.PathLike[str]] | None = None,
    policy: Mapping[str, object] | None = None,
    apply_env_policy: bool = True,
) -> TraceSession:
    """Start a new global trace session.

    Parameters
    ----------
    path:
        Destination directory for generated trace artefacts.
    format:
        Trace events serialisation format (``"binary"`` or ``"json"``).
    start_on_enter:
        Optional path that delays trace activation until the interpreter enters
        the referenced file.
    trace_filter:
        Optional filter specification. Accepts a path-like object, an iterable
        of path-like objects, or a string containing ``::``-separated paths.
        Paths are expanded to absolute locations and must exist.
    policy:
        Optional mapping of runtime policy overrides forwarded to
        :func:`configure_policy` before tracing begins. Keys match the policy
        keyword arguments (``on_recorder_error``, ``require_trace``, etc.).
    apply_env_policy:
        When ``True`` (default), refresh policy settings from environment
        variables via :func:`configure_policy_from_env` prior to applying
        explicit overrides.

    Returns
    -------
    TraceSession
        Handle for the active recorder session.

    Raises
    ------
    RecorderError
        Raised by the Rust backend when configuration, IO, or the target
        script fails.
    RuntimeError
        Raised when ``start`` is called while another session is still
        active. The guard lives in Python so the error stays synchronous.
    """
    global _active_session
    if _is_tracing_backend():
        raise RuntimeError("tracing already active")

    trace_path = _validate_trace_path(Path(path))
    normalized_format = _coerce_format(format)
    activation_path = _normalize_activation_path(start_on_enter)
    filter_chain = _normalize_trace_filter(trace_filter)

    if apply_env_policy:
        _configure_policy_from_env()
    if policy:
        _configure_policy(**_coerce_policy_kwargs(policy))

    _start_backend(str(trace_path), normalized_format, activation_path, filter_chain)
    session = TraceSession(path=trace_path, format=normalized_format)
    _active_session = session
    return session


def stop(*, exit_code: int | None = None) -> None:
    """Stop the active trace session if one is running.

    Parameters
    ----------
    exit_code:
        Optional process exit status to forward to the backend.
    """
    global _active_session
    if not _is_tracing_backend():
        return
    _stop_backend(exit_code)
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
    trace_filter: str | os.PathLike[str] | Sequence[str | os.PathLike[str]] | None = None,
    policy: Mapping[str, object] | None = None,
    apply_env_policy: bool = True,
) -> Iterator[TraceSession]:
    """Context manager helper for scoped tracing."""
    session = start(
        path,
        format=format,
        trace_filter=trace_filter,
        policy=policy,
        apply_env_policy=apply_env_policy,
    )
    try:
        yield session
    finally:
        session.stop()


def _coerce_format(value: str) -> str:
    normalized = normalize_format(value)
    if not is_supported(normalized):
        supported = ", ".join(sorted(SUPPORTED_FORMATS))
        raise ValueError(
            f"unsupported trace format '{value}'. Expected one of: {supported}"
        )
    return normalized


def _validate_trace_path(path: Path) -> Path:
    path = path.expanduser()
    if path.exists() and not path.is_dir():
        raise ValueError("trace path exists and is not a directory")
    return path


def _normalize_activation_path(value: str | Path | None) -> str | None:
    if value is None:
        return None
    return str(Path(value).expanduser())


def _normalize_trace_filter(
    value: str | os.PathLike[str] | Sequence[str | os.PathLike[str]] | None,
) -> list[str] | None:
    if value is None:
        return None

    segments = _extract_filter_segments(value)
    if not segments:
        raise ValueError("trace_filter must resolve to at least one path")

    resolved: list[str] = []
    for segment in segments:
        target = _resolve_trace_filter_path(segment)
        resolved.append(str(target))
    return resolved


def _extract_filter_segments(
    value: str | os.PathLike[str] | Sequence[str | os.PathLike[str]],
) -> list[str]:
    if isinstance(value, (str, os.PathLike)):
        return _split_filter_spec(os.fspath(value))

    if isinstance(value, Sequence):
        segments: list[str] = []
        for item in value:
            if not isinstance(item, (str, os.PathLike)):
                raise TypeError(
                    "trace_filter sequence entries must be str or os.PathLike"
                )
            segments.extend(_split_filter_spec(os.fspath(item)))
        return segments

    raise TypeError("trace_filter must be a path, iterable of paths, or None")


def _split_filter_spec(value: str) -> list[str]:
    parts = [segment.strip() for segment in value.split("::")]
    return [segment for segment in parts if segment]


def _resolve_trace_filter_path(raw: str) -> Path:
    candidate = Path(raw).expanduser()
    if not candidate.exists():
        raise FileNotFoundError(f"trace filter '{candidate}' does not exist")

    resolved = candidate.resolve()
    if not resolved.is_file():
        raise ValueError(f"trace filter '{resolved}' is not a file")
    return resolved


def _coerce_policy_kwargs(policy: Mapping[str, object]) -> dict[str, object]:
    normalized: dict[str, object] = {}
    for key, raw_value in policy.items():
        if key == "log_file" and raw_value is not None:
            normalized[key] = os.fspath(raw_value)
        elif key in {"on_recorder_error", "log_level"} and raw_value is not None:
            normalized[key] = str(raw_value)
        else:
            normalized[key] = raw_value
    return normalized


__all__ = (
    "TraceSession",
    "flush",
    "is_tracing",
    "start",
    "stop",
    "trace",
)
