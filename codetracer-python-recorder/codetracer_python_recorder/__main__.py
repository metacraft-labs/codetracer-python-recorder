"""CLI to record a trace while running a Python script.

Usage:
    python -m codetracer_python_recorder [codetracer options] <script.py> [script args...]

Codetracer options (must appear before the script path):
    --codetracer-trace PATH             Output events file (default: trace.bin or trace.json)
    --codetracer-format {binary,json}   Output format (default: binary)
    --codetracer-on-recorder-error MODE How to react to recorder errors (abort|disable)
    --codetracer-require-trace          Exit with failure if no trace is produced
    --codetracer-keep-partial-trace     Preserve partial traces when failures occur
    --codetracer-log-level LEVEL        Override Rust log filter (e.g. info,debug)
    --codetracer-log-file PATH          Write recorder logs to a file
    --codetracer-json-errors            Emit JSON error trailers on stderr

Examples:
    python -m codetracer_python_recorder --codetracer-format=json app.py --flag=1
    python -m codetracer_python_recorder --codetracer-trace=out.bin script.py --x=2
    python -m codetracer_python_recorder --codetracer-capture-values=false script.py
"""
from __future__ import annotations

import runpy
import sys
from pathlib import Path

from . import DEFAULT_FORMAT, configure_policy, configure_policy_from_env, start, stop
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
    parser.add_argument(
        "--codetracer-on-recorder-error",
        dest="on_recorder_error",
        choices=["abort", "disable"],
        help="How the recorder responds to internal errors (abort or disable).",
    )
    parser.add_argument(
        "--codetracer-require-trace",
        dest="require_trace",
        action="store_true",
        help="Exit with status 1 if no trace output is produced.",
    )
    parser.add_argument(
        "--codetracer-keep-partial-trace",
        dest="keep_partial_trace",
        action="store_true",
        help="Keep partial trace files on failure instead of cleaning up.",
    )
    parser.add_argument(
        "--codetracer-log-level",
        dest="log_level",
        default=None,
        help="Override the Rust log filter (e.g. info,debug).",
    )
    parser.add_argument(
        "--codetracer-log-file",
        dest="log_file",
        default=None,
        help="Path to a file where recorder logs should be written.",
    )
    parser.add_argument(
        "--codetracer-json-errors",
        dest="json_errors",
        action="store_true",
        help="Emit JSON error trailers on stderr for machine parsing.",
    )

    configure_policy_from_env()
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

    policy_kwargs: dict[str, object] = {}
    if ns.on_recorder_error:
        policy_kwargs["on_recorder_error"] = ns.on_recorder_error
    if ns.require_trace:
        policy_kwargs["require_trace"] = True
    if ns.keep_partial_trace:
        policy_kwargs["keep_partial_trace"] = True
    if ns.log_level is not None:
        policy_kwargs["log_level"] = ns.log_level
    if ns.log_file is not None:
        policy_kwargs["log_file"] = ns.log_file
    if ns.json_errors:
        policy_kwargs["json_errors"] = True
    if policy_kwargs:
        configure_policy(**policy_kwargs)

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
