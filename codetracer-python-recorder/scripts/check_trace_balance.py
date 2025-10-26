"""CLI helper to verify that a Codetracer ``trace.json`` stream is balanced."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path
from typing import Sequence

from codetracer_python_recorder.trace_balance import (
    TraceBalanceError,
    load_trace_events,
    summarize_trace_balance,
)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Check whether a Codetracer trace.json has matching call and return events.",
    )
    parser.add_argument(
        "trace",
        type=Path,
        help="Path to trace.json emitted by Codetracer",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    trace_path = args.trace

    try:
        events = load_trace_events(trace_path)
        result = summarize_trace_balance(events)
    except TraceBalanceError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    if result.is_balanced:
        print(
            f"Balanced trace: {result.call_count} call events, "
            f"{result.return_count} return events."
        )
        return 0

    print("Unbalanced trace detected.")
    print(f"Call events   : {result.call_count}")
    print(f"Return events : {result.return_count}")

    delta = result.delta
    if delta > 0:
        print(f"Missing {delta} return event(s).")
    elif delta < 0:
        print(f"Unexpected {-delta} extra return event(s).")

    if result.first_negative_index is not None:
        print(
            "The first unmatched return appears at event "
            f"#{result.first_negative_index} (0-based index)."
        )

    return 1


if __name__ == "__main__":
    sys.exit(main())
