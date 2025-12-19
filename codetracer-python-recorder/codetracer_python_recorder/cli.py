"""Command-line interface for the Codetracer Python recorder."""
from __future__ import annotations

import argparse
import json
import os
import runpy
import sys
from dataclasses import dataclass
from importlib import metadata
from pathlib import Path
from typing import Iterable, Sequence

from . import flush, policy_snapshot, start, stop
from .auto_start import ENV_TRACE_FILTER
from .formats import DEFAULT_FORMAT, SUPPORTED_FORMATS, normalize_format


@dataclass(frozen=True)
class RecorderCLIConfig:
    """Resolved CLI options for a recorder invocation."""

    trace_dir: Path
    format: str
    activation_path: Path | None
    script: Path | None
    script_args: list[str]
    trace_filter: tuple[str, ...]
    policy_overrides: dict[str, object]
    # Test framework support
    pytest_args: list[str] | None
    unittest_args: list[str] | None
    no_framework_filters: bool


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
    parser.add_argument(
        "--trace-filter",
        action="append",
        help=(
            "Path to a trace filter file. Provide multiple times to chain filters; "
            "specify multiple paths within a single argument using '::' separators. "
            "Filters load after any project default '.codetracer/trace-filter.toml' so "
            "later entries override earlier ones; the CODETRACER_TRACE_FILTER "
            "environment variable accepts the same syntax for env auto-start."
        ),
    )
    parser.add_argument(
        "--on-recorder-error",
        choices=["abort", "disable"],
        help=(
            "How the recorder reacts to internal errors. "
            "'abort' propagates the failure, 'disable' stops tracing but keeps the script running."
        ),
    )
    parser.add_argument(
        "--require-trace",
        action="store_true",
        help="Exit with status 1 if no trace artefacts are produced.",
    )
    parser.add_argument(
        "--keep-partial-trace",
        action="store_true",
        help="Preserve partially written trace artefacts when failures occur.",
    )
    parser.add_argument(
        "--log-level",
        help="Override the recorder log verbosity (examples: info, debug).",
    )
    parser.add_argument(
        "--log-file",
        type=Path,
        help="Write recorder logs to the specified file instead of stderr.",
    )
    parser.add_argument(
        "--json-errors",
        action="store_true",
        help="Emit JSON error trailers on stderr.",
    )
    parser.add_argument(
        "--io-capture",
        choices=["off", "proxies", "proxies+fd"],
        help=(
            "Control stdout/stderr capture. Without this flag, line-aware proxies stay enabled. "
            "'off' disables capture, 'proxies' forces proxies without FD mirroring, "
            "'proxies+fd' also mirrors raw file-descriptor writes."
        ),
    )
    parser.add_argument(
        "--module-name-from-globals",
        action=argparse.BooleanOptionalAction,
        default=None,
        help=(
            "Derive module names from the Python frame's __name__ attribute (default: enabled). "
            "Use '--no-module-name-from-globals' to fall back to the legacy resolver."
        ),
    )
    parser.add_argument(
        "--propagate-script-exit",
        action=argparse.BooleanOptionalAction,
        default=None,
        help=(
            "Mirror the traced script's exit status when the recorder succeeds (default: disabled). "
            "Use '--no-propagate-script-exit' to force a zero exit status."
        ),
    )

    # Test framework support - mutually exclusive with script mode
    framework_group = parser.add_mutually_exclusive_group()
    framework_group.add_argument(
        "--pytest",
        nargs=argparse.REMAINDER,
        metavar="ARGS",
        help=(
            "Run pytest with the specified arguments. Everything after --pytest is "
            "passed directly to pytest. Automatically applies pytest-specific filters. "
            "Example: --pytest tests/test_foo.py::test_bar -v"
        ),
    )
    framework_group.add_argument(
        "--unittest",
        nargs=argparse.REMAINDER,
        metavar="ARGS",
        help=(
            "Run unittest with the specified arguments. Everything after --unittest is "
            "passed directly to unittest. Automatically applies unittest-specific filters. "
            "Example: --unittest discover -s tests"
        ),
    )
    parser.add_argument(
        "--no-framework-filters",
        action="store_true",
        help=(
            "Disable automatic framework-specific filters when using --pytest or --unittest. "
            "Only explicit --trace-filter arguments and project defaults will be applied."
        ),
    )

    known, remainder = parser.parse_known_args(argv)

    # Determine execution mode: pytest, unittest, or script
    pytest_args: list[str] | None = None
    unittest_args: list[str] | None = None
    script_path: Path | None = None
    script_args: list[str] = []

    if known.pytest is not None:
        # Pytest mode - all args after --pytest go to pytest
        pytest_args = known.pytest
        if not pytest_args:
            parser.error("--pytest requires at least one argument (test path or options)")
    elif known.unittest is not None:
        # Unittest mode - all args after --unittest go to unittest
        unittest_args = known.unittest
        if not unittest_args:
            parser.error("--unittest requires at least one argument")
    else:
        # Script mode - parse remaining args as script + script_args
        pending: list[str] = list(remainder)
        if not pending:
            parser.error("missing script to execute (or use --pytest/--unittest for test frameworks)")

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

    activation_path: Path | None = None
    if known.activation_path:
        activation_path = Path(known.activation_path).expanduser().resolve()
    elif script_path:
        activation_path = script_path

    policy: dict[str, object] = {}
    if known.on_recorder_error:
        policy["on_recorder_error"] = known.on_recorder_error
    if known.require_trace:
        policy["require_trace"] = True
    if known.keep_partial_trace:
        policy["keep_partial_trace"] = True
    if known.log_level:
        policy["log_level"] = known.log_level
    if known.log_file is not None:
        policy["log_file"] = Path(known.log_file).expanduser().resolve()
    if known.json_errors:
        policy["json_errors"] = True
    if known.io_capture:
        match known.io_capture:
            case "off":
                policy["io_capture_line_proxies"] = False
                policy["io_capture_fd_fallback"] = False
            case "proxies":
                policy["io_capture_line_proxies"] = True
                policy["io_capture_fd_fallback"] = False
            case "proxies+fd":
                policy["io_capture_line_proxies"] = True
                policy["io_capture_fd_fallback"] = True
            case other:  # pragma: no cover - argparse choices block this
                parser.error(f"unsupported io-capture mode '{other}'")
    if known.module_name_from_globals is not None:
        policy["module_name_from_globals"] = known.module_name_from_globals
    if known.propagate_script_exit is not None:
        policy["propagate_script_exit"] = known.propagate_script_exit

    return RecorderCLIConfig(
        trace_dir=trace_dir,
        format=fmt,
        activation_path=activation_path,
        script=script_path,
        script_args=script_args,
        trace_filter=tuple(known.trace_filter or ()),
        policy_overrides=policy,
        pytest_args=pytest_args,
        unittest_args=unittest_args,
        no_framework_filters=known.no_framework_filters,
    )


def _resolve_package_version() -> str | None:
    try:
        return metadata.version("codetracer-python-recorder")
    except metadata.PackageNotFoundError:  # pragma: no cover - dev checkout
        return None


def _serialise_metadata(
    trace_dir: Path,
    *,
    script: Path | None = None,
    test_framework: str | None = None,
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
        if script:
            recorder_block["target_script"] = str(script)
        if test_framework:
            recorder_block["test_framework"] = test_framework
        version = _resolve_package_version()
        if version:
            recorder_block["version"] = version
    else:
        # Unexpected schema — bail out without mutating further.
        return

    metadata_path.write_text(json.dumps(payload), encoding="utf-8")


def _run_pytest(pytest_args: list[str]) -> int:
    """Run pytest with the given arguments and return its exit code."""
    try:
        import pytest
    except ImportError:
        sys.stderr.write(
            "pytest is not installed. Install it with: pip install pytest\n"
        )
        return 1

    return pytest.main(pytest_args)


def _run_unittest(unittest_args: list[str]) -> int:
    """Run unittest with the given arguments and return its exit code."""
    import unittest

    # unittest.main() expects args in sys.argv format
    old_argv = sys.argv
    sys.argv = ["unittest"] + unittest_args
    try:
        # Use exit=False to prevent SystemExit and get proper return
        program = unittest.main(module=None, exit=False)
        return 0 if program.result.wasSuccessful() else 1
    except SystemExit as e:
        return e.code if isinstance(e.code, int) else 1
    finally:
        sys.argv = old_argv


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
    filter_specs = list(config.trace_filter)
    env_filter = os.getenv(ENV_TRACE_FILTER)
    if env_filter:
        filter_specs.insert(0, env_filter)
    policy_overrides = config.policy_overrides if config.policy_overrides else None

    # Determine execution mode and test framework
    test_framework: str | None = None
    if config.pytest_args is not None:
        test_framework = "pytest"
    elif config.unittest_args is not None:
        test_framework = "unittest"

    # Set up sys.argv for the execution
    old_argv = sys.argv
    if config.script:
        sys.argv = [str(config.script)] + config.script_args
    elif config.pytest_args is not None:
        sys.argv = ["pytest"] + config.pytest_args
    elif config.unittest_args is not None:
        sys.argv = ["unittest"] + config.unittest_args

    try:
        start(
            trace_dir,
            format=config.format,
            start_on_enter=config.activation_path,
            trace_filter=filter_specs or None,
            policy=policy_overrides,
            test_framework=test_framework if not config.no_framework_filters else None,
        )
    except Exception as exc:
        sys.stderr.write(f"Failed to start Codetracer session: {exc}\n")
        sys.argv = old_argv
        return 1

    snapshot = policy_snapshot()
    propagate_script_exit = bool(snapshot.get("propagate_script_exit"))

    exit_code: int | None = None
    recorder_failed = False
    try:
        try:
            # Execute based on mode
            if config.pytest_args is not None:
                exit_code = _run_pytest(config.pytest_args)
            elif config.unittest_args is not None:
                exit_code = _run_unittest(config.unittest_args)
            elif config.script:
                runpy.run_path(str(config.script), run_name="__main__")
                exit_code = 0
            else:
                # Should not happen due to argument validation
                sys.stderr.write("No execution mode specified\n")
                exit_code = 1
        except SystemExit as exc:
            exit_code = exc.code if isinstance(exc.code, int) else 1
    finally:
        try:
            flush()
        except Exception as exc:
            recorder_failed = True
            sys.stderr.write(f"Failed to flush Codetracer session: {exc}\n")
        finally:
            try:
                stop(exit_code=exit_code)
            except Exception as exc:
                recorder_failed = True
                sys.stderr.write(f"Failed to stop Codetracer session: {exc}\n")
            finally:
                sys.argv = old_argv

    _serialise_metadata(trace_dir, script=config.script, test_framework=test_framework)

    script_exit_code = exit_code if exit_code is not None else 0

    if recorder_failed:
        return 1

    if propagate_script_exit:
        return script_exit_code

    if script_exit_code != 0:
        sys.stderr.write(
            f"Script exited with status {script_exit_code}; returning 0. "
            "Use '--propagate-script-exit' to mirror the script exit code.\n"
        )

    return 0


__all__ = ("main", "RecorderCLIConfig")
