"""Utilities for validating the balance of Codetracer JSON trace files.

A trace is considered *balanced* when it contains the same number of
``Call`` and ``Return`` events and the stream of events never dips below
zero (i.e. we never see a return without a preceding call).  This helper
module provides small, importable utilities that can be reused by the
CLI helper script as well as unit tests.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence


class TraceBalanceError(RuntimeError):
    """Raised when the trace file is malformed or cannot be processed."""


@dataclass(frozen=True)
class TraceBalanceResult:
    """Summary information about call/return balance within a trace."""

    call_count: int
    return_count: int
    first_negative_index: int | None

    @property
    def is_balanced(self) -> bool:
        """Return True when the trace has matching call/return counts and never underflows."""
        return self.call_count == self.return_count and self.first_negative_index is None

    @property
    def delta(self) -> int:
        """Number of call events minus return events."""
        return self.call_count - self.return_count


def load_trace_events(trace_path: Path) -> Sequence[Mapping[str, Any]]:
    """Load and validate the JSON event stream from ``trace.json``."""
    try:
        raw_text = trace_path.read_text(encoding="utf-8")
    except FileNotFoundError as exc:
        raise TraceBalanceError(f"trace file not found: {trace_path}") from exc
    except OSError as exc:  # pragma: no cover - surfaced in tests via mocked IO
        raise TraceBalanceError(f"unable to read trace file: {trace_path}") from exc

    try:
        data = json.loads(raw_text)
    except json.JSONDecodeError as exc:
        raise TraceBalanceError(f"invalid JSON in trace file: {trace_path}: {exc}") from exc

    if not isinstance(data, list):
        raise TraceBalanceError(f"trace root must be a JSON array, got {type(data).__name__}")

    for index, event in enumerate(data):
        if not isinstance(event, dict):
            raise TraceBalanceError(
                f"event #{index} is not a JSON object (found {type(event).__name__})"
            )

    return data


def summarize_trace_balance(events: Iterable[Mapping[str, Any]]) -> TraceBalanceResult:
    """Return a balance summary for the provided event sequence."""
    calls = 0
    returns = 0
    active_calls = 0
    first_negative_index: int | None = None

    for index, event in enumerate(events):
        is_call = "Call" in event
        is_return = "Return" in event

        if is_call and is_return:
            raise TraceBalanceError(
                f"event #{index} contains both Call and Return payloads, which is unsupported"
            )

        if is_call:
            calls += 1
            active_calls += 1
        if is_return:
            returns += 1
            active_calls -= 1
            if active_calls < 0 and first_negative_index is None:
                first_negative_index = index

    return TraceBalanceResult(
        call_count=calls,
        return_count=returns,
        first_negative_index=first_negative_index,
    )


__all__ = [
    "TraceBalanceError",
    "TraceBalanceResult",
    "load_trace_events",
    "summarize_trace_balance",
]
