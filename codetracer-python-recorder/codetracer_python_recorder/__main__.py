"""CLI to record a trace while running a Python script.

Usage:
    python -m codetracer_python_recorder [codetracer options] <script.py> [script args...]

Codetracer options (must appear before the script path):
    --codetracer-trace PATH             Output events file (default: trace.bin or trace.json)
    --codetracer-format {binary,json}   Output format (default: binary)
    --codetracer-capture-values BOOL    Whether to capture values (default: true)

Examples:
    python -m codetracer_python_recorder --codetracer-format=json app.py --flag=1
    python -m codetracer_python_recorder --codetracer-trace=out.bin script.py --x=2
    python -m codetracer_python_recorder --codetracer-capture-values=false script.py
"""
from __future__ import annotations

import runpy
import sys
from pathlib import Path

from . import DEFAULT_FORMAT, start, stop
import argparse


def _default_trace_path(fmt: str) -> Path:
    # Keep a simple filename; Rust side derives sidecars (metadata/paths)
    if fmt == "json":
        return Path.cwd() / "trace.json"
    return Path.cwd() / "trace.bin"


def main(argv: list[str] | None = None) -> int:
    if argv is None:
        argv = sys.argv[1:]

    parser = argparse.ArgumentParser(add_help=True)
    parser.add_argument(
        "--codetracer-trace",
        dest="trace",
        default=None,
        help="Path to trace folder. If omitted, defaults to trace.bin or trace.json in the current directory based on --codetracer-format.",
    )
    parser.add_argument(
        "--codetracer-format",
        dest="format",
        choices=["binary", "json"],
        default=DEFAULT_FORMAT,
        help="Output format for trace events. 'binary' is compact; 'json' is human-readable. Default: %(default)s.",
    )
    # Only parse our options; leave script and script args in unknown
    ns, unknown = parser.parse_known_args(argv)

    # Validate that the first unknown token is a script path; otherwise show usage.
    if not unknown or not Path(unknown[0]).exists():
        sys.stderr.write("Usage: python -m codetracer_python_recorder [codetracer options] <script.py> [args...]\n")
        return 2

    script_path = Path(unknown[0]).resolve()
    script_args = unknown[1:]

    fmt = ns.format or DEFAULT_FORMAT
    trace_path = Path(ns.trace) if ns.trace else _default_trace_path(fmt)

    old_argv = sys.argv
    sys.argv = [str(script_path)] + script_args
    # Activate tracing only after entering the target script file.
    session = start(
        trace_path,
        format=fmt,
        start_on_enter=script_path,
    )
    try:
        runpy.run_path(str(script_path), run_name="__main__")
        return 0
    except SystemExit as e:
        # Preserve script's exit code
        code = e.code if isinstance(e.code, int) else 1
        return code
    finally:
        # Ensure tracer stops and files are flushed
        try:
            session.flush()
        finally:
            stop()
            sys.argv = old_argv


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
