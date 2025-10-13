"""Command-line interface for the Codetracer Python recorder."""
from __future__ import annotations

import argparse
import json
import runpy
import sys
from dataclasses import dataclass
from importlib import metadata
from pathlib import Path
from typing import Iterable, Sequence

from . import flush, start, stop
from .formats import DEFAULT_FORMAT, SUPPORTED_FORMATS, normalize_format


@dataclass(frozen=True)
class RecorderCLIConfig:
    """Resolved CLI options for a recorder invocation."""

    trace_dir: Path
    format: str
    activation_path: Path
    script: Path
    script_args: list[str]


def _default_trace_dir() -> Path:
    return Path.cwd() / "trace-out"


def _parse_args(argv: Sequence[str]) -> RecorderCLIConfig:
    parser = argparse.ArgumentParser(
        prog="codetracer_python_recorder",
        description=(
            "Record a trace for a Python script using the Codetracer runtime tracer. "
            "All script arguments must be provided after the script path or a '--' separator."
        ),
        allow_abbrev=False,
    )
    parser.add_argument(
        "--trace-dir",
        type=Path,
        default=_default_trace_dir(),
        help=(
            "Directory where trace artefacts will be written "
            "(defaults to %(default)s relative to the current working directory)."
        ),
    )
    parser.add_argument(
        "--format",
        default=DEFAULT_FORMAT,
        help=(
            "Trace serialisation format. Supported values: "
            + ", ".join(sorted(SUPPORTED_FORMATS))
            + f". Defaults to {DEFAULT_FORMAT}."
        ),
    )
    parser.add_argument(
        "--activation-path",
        type=Path,
        help=(
            "Optional path used to gate tracing. When provided, tracing begins once the "
            "interpreter enters this file. Defaults to the target script."
        ),
    )

    known, remainder = parser.parse_known_args(argv)
    pending: list[str] = list(remainder)
    if not pending:
        parser.error("missing script to execute")

    if pending[0] == "--":
        pending.pop(0)
        if not pending:
            parser.error("missing script path after '--'")

    script_token = pending[0]
    script_path = Path(script_token).expanduser()
    if not script_path.exists():
        parser.error(f"script '{script_path}' does not exist")
    script_path = script_path.resolve()

    script_args = pending[1:]
    if script_args and script_args[0] == "--":
        script_args = script_args[1:]

    trace_dir = Path(known.trace_dir).expanduser().resolve()
    fmt = normalize_format(known.format)
    if fmt not in SUPPORTED_FORMATS:
        parser.error(
            f"unsupported trace format '{known.format}'. Expected one of: "
            + ", ".join(sorted(SUPPORTED_FORMATS))
        )

    activation_path = (
        Path(known.activation_path).expanduser().resolve()
        if known.activation_path
        else script_path
    )

    return RecorderCLIConfig(
        trace_dir=trace_dir,
        format=fmt,
        activation_path=activation_path,
        script=script_path,
        script_args=script_args,
    )


def _resolve_package_version() -> str | None:
    try:
        return metadata.version("codetracer-python-recorder")
    except metadata.PackageNotFoundError:  # pragma: no cover - dev checkout
        return None


def _serialise_metadata(
    trace_dir: Path,
    *,
    script: Path,
) -> None:
    """Augment trace metadata with recorder-specific information."""
    metadata_path = trace_dir / "trace_metadata.json"
    try:
        raw = metadata_path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return

    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        return

    recorder_block = payload.setdefault(
        "recorder",
        {
            "name": "codetracer_python_recorder",
        },
    )
    if isinstance(recorder_block, dict):
        recorder_block.setdefault("name", "codetracer_python_recorder")
        recorder_block["target_script"] = str(script)
        version = _resolve_package_version()
        if version:
            recorder_block["version"] = version
    else:
        # Unexpected schema — bail out without mutating further.
        return

    metadata_path.write_text(json.dumps(payload), encoding="utf-8")


def main(argv: Iterable[str] | None = None) -> int:
    """Entry point for ``python -m codetracer_python_recorder``."""
    if argv is None:
        argv = sys.argv[1:]

    try:
        config = _parse_args(list(argv))
    except SystemExit:
        # argparse already printed a helpful message; propagate exit code.
        raise
    except Exception as exc:  # pragma: no cover - defensive guardrail
        sys.stderr.write(f"Failed to parse arguments: {exc}\n")
        return 2

    trace_dir = config.trace_dir
    script_path = config.script
    script_args = config.script_args

    old_argv = sys.argv
    sys.argv = [str(script_path)] + script_args

    try:
        start(
            trace_dir,
            format=config.format,
            start_on_enter=config.activation_path,
        )
    except Exception as exc:
        sys.stderr.write(f"Failed to start Codetracer session: {exc}\n")
        sys.argv = old_argv
        return 1

    exit_code: int | None = None
    try:
        try:
            runpy.run_path(str(script_path), run_name="__main__")
        except SystemExit as exc:
            exit_code = exc.code if isinstance(exc.code, int) else 1
        else:
            exit_code = 0
    finally:
        try:
            flush()
        finally:
            stop()
            sys.argv = old_argv

    _serialise_metadata(trace_dir, script=script_path)

    return exit_code if exit_code is not None else 0


__all__ = ("main", "RecorderCLIConfig")
