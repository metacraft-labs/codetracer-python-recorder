"""Trace format constants and helpers."""
from __future__ import annotations

from typing import Iterable

TRACE_BINARY: str = "binary"
TRACE_CTFS: str = "ctfs"
DEFAULT_FORMAT: str = TRACE_CTFS
SUPPORTED_FORMATS: frozenset[str] = frozenset({TRACE_BINARY, TRACE_CTFS})


def normalize_format(value: str | None) -> str:
    """Normalise user-provided strings to the format names recognised by the backend.

    The runtime accepts ``"ctfs"`` (the default) and ``"binary"`` (plus
    legacy aliases handled on the Rust side). There is no JSON trace
    format: JSON traces come only from the pure-Python test oracle, and
    CodeTracer cannot open them.
    Unknown formats fall back to the lower-cased input so the backend can
    decide how to react; callers can choose to guard against unsupported
    values by checking ``SUPPORTED_FORMATS``.
    """
    if value is None:
        return DEFAULT_FORMAT
    return value.lower()


def is_supported(value: str) -> bool:
    """Return ``True`` if *value* is one of the officially supported formats."""
    return value.lower() in SUPPORTED_FORMATS


__all__: Iterable[str] = (
    "DEFAULT_FORMAT",
    "TRACE_BINARY",
    "TRACE_CTFS",
    "SUPPORTED_FORMATS",
    "is_supported",
    "normalize_format",
)
