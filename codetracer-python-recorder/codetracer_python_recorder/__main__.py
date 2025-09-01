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

    parser = argparse.ArgumentParser(add_help=False, allow_abbrev=False)
    parser.add_argument("--codetracer-trace", dest="trace", default=None)
    parser.add_argument(
        "--codetracer-format",
        dest="format",
        choices=["binary", "json"],
        default=DEFAULT_FORMAT,
    )
    # Only parse our options; leave script and script args in unknown
    ns, unknown = parser.parse_known_args(argv)

    # If unknown contains codetracer-prefixed options, they are invalid
    for tok in unknown:
        if isinstance(tok, str) and tok.startswith("--codetracer-"):
            sys.stderr.write(f"error: unknown codetracer option: {tok}\n")
            return 2

    if not unknown:
        sys.stderr.write("Usage: python -m codetracer_python_recorder [codetracer options] <script.py> [args...]\n")
        return 2

    script_path = Path(unknown[0])
    script_args = unknown[1:]

    if not script_path.exists():
        sys.stderr.write(f"error: script not found: {script_path}\n")
        return 2

    def _parse_bool(s: str) -> bool:
        v = s.strip().lower()
        if v in {"1", "true", "yes", "on"}:
            return True
        if v in {"0", "false", "no", "off"}:
            return False
        raise ValueError(f"invalid boolean: {s}")

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
        runpy.run_path(str(script_path.resolve()), run_name="__main__")
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
