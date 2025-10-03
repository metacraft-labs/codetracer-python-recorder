"""Shared helpers for Python test suites."""
from __future__ import annotations

from pathlib import Path

__all__ = ["ensure_trace_dir"]


def ensure_trace_dir(root: Path, name: str = "trace_out") -> Path:
    """Return an existing trace directory under ``root``, creating it if needed."""
    target = root / name
    target.mkdir(exist_ok=True)
    return target

