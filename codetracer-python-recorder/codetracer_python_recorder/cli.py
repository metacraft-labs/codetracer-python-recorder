"""Command-line interface for the Codetracer Python recorder.

The CLI is CTFS-only per ``codetracer-specs/Recorder-CLI-Conventions.md``
§4: there is no ``--format`` flag and no ``CODETRACER_FORMAT`` environment
variable. Use ``ct print`` (shipped with ``codetracer-trace-format-nim``)
to convert recorded CTFS traces to JSON or other human-readable forms.
"""
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
from .formats import DEFAULT_FORMAT


# Per ``Recorder-CLI-Conventions.md`` §5, every recorder reads
# ``CODETRACER_<LANG>_RECORDER_OUT_DIR`` as a fallback for ``--out-dir``
# and ``CODETRACER_<LANG>_RECORDER_DISABLED`` to skip recording entirely.
# These complement (and do not replace) the ``CODETRACER_TRACE``
# auto-start path env var defined in ``auto_start.py``.
ENV_OUT_DIR = "CODETRACER_PYTHON_RECORDER_OUT_DIR"
ENV_DISABLED = "CODETRACER_PYTHON_RECORDER_DISABLED"


@dataclass(frozen=True)
class RecorderCLIConfig:
    """Resolved CLI options for a recorder invocation."""

    out_dir: Path
    activation_path: Path | None
    script: Path | None
    script_args: list[str]
    trace_filter: tuple[str, ...]
    policy_overrides: dict[str, object]
    # Test framework support
    pytest_args: list[str] | None
    unittest_args: list[str] | None
    no_framework_filters: bool
    # P6.2: recorder-side autoformat of minified sources (default on;
    # ``--no-autoformat`` opts out).  Threaded into the session config
    # once the record-cmd hook lands in the runtime tracer.
    autoformat: bool = True

    @property
    def format(self) -> str:
        """The trace format is hard-pinned to CTFS — see module docstring."""
        return DEFAULT_FORMAT


def _default_trace_dir() -> Path:
    return Path.cwd() / "trace-out"


def resolve_out_dir(cli_value: Path | None) -> Path:
    """Resolve the output directory using CLI flag → env var → default.

    Per ``Recorder-CLI-Conventions.md`` §5, CLI flags always take
    precedence over environment variables. The fallback chain is:

      1. ``cli_value`` (the value of ``--out-dir`` / ``-o``).
      2. ``CODETRACER_PYTHON_RECORDER_OUT_DIR`` from the environment.
      3. ``./trace-out`` relative to the current working directory.
    """
    if cli_value is not None:
        return Path(cli_value).expanduser().resolve()

    env_value = os.getenv(ENV_OUT_DIR)
    if env_value:
        return Path(env_value).expanduser().resolve()

    return _default_trace_dir().resolve()


def recording_disabled() -> bool:
    """Return ``True`` when ``CODETRACER_PYTHON_RECORDER_DISABLED`` is set.

    Accepts ``1`` and ``true`` (case-insensitive) as truthy values.  Any
    other value (including unset) means recording is enabled.
    """
    raw = os.getenv(ENV_DISABLED)
    if raw is None:
        return False
    return raw.strip().lower() in ("1", "true")


def _help_epilog() -> str:
    """Help text appended to ``--help`` output.

    Documents the env-var fallbacks (per convention §5) and points users
    at ``ct print`` for human-readable conversion (per convention §4).

    The text deliberately avoids the literal strings ``--format`` and
    ``CODETRACER_FORMAT`` so the convention verifier (which checks for
    their absence in ``--help``) keeps passing.  Users who go looking
    for ``--format`` in the help will be redirected by the explicit
    parser.error in ``_parse_args`` if they actually try the flag.
    """
    return (
        "Output format:\n"
        "  The recorder always writes the canonical CTFS trace format. There is no\n"
        "  format-selector flag or environment variable. To convert a recorded CTFS\n"
        "  trace to JSON or text for inspection or golden snapshot fixtures, run\n"
        "  `ct print` (shipped with codetracer-trace-format-nim) on the produced\n"
        "  trace.\n"
        "\n"
        "Environment variables:\n"
        f"  {ENV_OUT_DIR}    Default output directory (overridden by --out-dir).\n"
        f"  {ENV_DISABLED}    Set to 1 or true to skip recording entirely.\n"
        "  CODETRACER_TRACE_FILTER       Trace filter chain for env auto-start.\n"
        "  CODETRACER_TRACE              Auto-start trace path (when imported as a library).\n"
    )


def _parse_args(argv: Sequence[str]) -> RecorderCLIConfig:
    parser = argparse.ArgumentParser(
        prog="codetracer-python-recorder",
        description=(
            "Record a CTFS trace for a Python script using the Codetracer runtime tracer. "
            "All script arguments must be provided after the script path or a '--' separator. "
            "Use `ct print` to convert the recorded trace to JSON or text."
        ),
        epilog=_help_epilog(),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        allow_abbrev=False,
    )
    parser.add_argument(
        "--version",
        "-V",
        action="version",
        version=f"codetracer-python-recorder {_resolve_package_version() or 'dev'}",
    )
    parser.add_argument(
        "--out-dir",
        "-o",
        type=Path,
        default=None,
        help=(
            "Directory where CTFS trace artefacts will be written "
            "(defaults to ./trace-out, or the value of "
            f"{ENV_OUT_DIR} when set)."
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
    # P6.2 (Column-Aware-Tracing-And-Deminification milestone):
    # recorder-side autoformat of minified Python sources via ``black``.
    # Defaults to ``True`` so the recorder matches the JS recorder's
    # default-on behaviour for minified-bundle pre-formatting; users
    # who don't want the formatted sibling materialised can disable
    # via ``--no-autoformat`` or by setting ``CT_AUTOFORMAT=0`` (the
    # env var the replay-server's lazy P4 fallback also reads).
    # See ``src/runtime/autoformat.rs`` for the heuristic + ``black``
    # invocation details; flag is surfaced here so it threads through
    # the recorder session config once the record-cmd hook lands.
    parser.add_argument(
        "--autoformat",
        action=argparse.BooleanOptionalAction,
        default=True,
        help=(
            "Pre-format minified Python sources at record time via 'black' "
            "(default: on). Use '--no-autoformat' to record minified sources "
            "unformatted; the replay-server's P4 fallback can still format at "
            "view time on machines that have 'black'. Also disabled when "
            "CT_AUTOFORMAT is set to 0/off/false/no."
        ),
    )
    # P0.2 (Performance + E2E Coverage): client-controlled omniscient-DB
    # upload mode. The flag is forwarded to the Monolith's finalize body
    # as the camelCase ``omniscientDbMode`` field per CS-M7. The Python
    # recorder doesn't drive the finalize HTTP itself (the managed-upload
    # path delegates to the Rust ``codetracer_ctfs`` crate); the flag is
    # surfaced here so the CLI accepts the spec-conformant argument and
    # ``codetracer_python_recorder`` can forward it to the upload layer
    # via the ``CODETRACER_OMNISCIENT_DB_MODE`` environment variable. The
    # actual HTTP wire-up lands at the upload site once the
    # ``managed_upload_materialized_trace`` PyO3 binding accepts a mode
    # parameter (deferred; the env-var path keeps the CLI honest today).
    parser.add_argument(
        "--omniscient-db",
        choices=["off", "on", "lazy", "pre-prepared"],
        default=None,
        help=(
            "Omniscient-DB upload mode forwarded to the Monolith as the camelCase "
            "'omniscientDbMode' finalize-body field (CS-M7). 'off' is the default; "
            "'on' eagerly builds the omniscient namespaces server-side; 'lazy' "
            "defers the build until the first omniscient query; 'pre-prepared' "
            "uploads the namespaces inline."
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

    # ``parse_known_args`` is intentionally kept so script-mode trailing
    # arguments fall through into ``remainder``; an unknown flag in
    # script mode is treated as a script argument.  The ``--format``
    # flag is therefore *not* silently swallowed: see
    # ``test_format_flag_rejected`` for the error contract.
    known, remainder = parser.parse_known_args(argv)

    # Per Recorder-CLI-Conventions.md §4, the recorder is CTFS-only.  We
    # explicitly reject any leftover ``--format`` flag (which used to be
    # accepted in pre-2026-05 versions) so callers see a clear failure
    # rather than silently writing CTFS while believing they asked for
    # JSON.  We check the original argv (not the remainder) because in
    # script mode argparse may push ``--format`` past the script path
    # into remainder, and we want to reject it regardless of position.
    if any(arg == "--format" or arg.startswith("--format=") for arg in argv):
        parser.error(
            "the --format flag has been removed: the recorder always writes CTFS. "
            "Use `ct print` to convert the trace to JSON or text."
        )

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

    trace_dir = resolve_out_dir(known.out_dir)

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

    # P0.2 (Performance + E2E Coverage): set the env-var bridge so the
    # managed-upload session (codetracer_python_recorder.session) and
    # downstream Rust upload layer can forward the omniscient-DB mode
    # to the Monolith's CS-M7 finalize body. Doing this here keeps the
    # CLI spec-conformant ahead of the PyO3 binding accepting a mode
    # parameter directly.
    if known.omniscient_db is not None:
        os.environ["CODETRACER_OMNISCIENT_DB_MODE"] = known.omniscient_db

    # P6.2: ``--no-autoformat`` overrides the default-on behaviour at
    # the CLI level.  Even when the flag says "on", the recorder still
    # respects ``CT_AUTOFORMAT=0/off/false/no`` at the runtime level
    # via ``autoformat_enabled_by_env`` so deployment environments can
    # disable globally without touching every recorder invocation.
    autoformat_flag = bool(known.autoformat) if known.autoformat is not None else True

    return RecorderCLIConfig(
        out_dir=trace_dir,
        activation_path=activation_path,
        script=script_path,
        script_args=script_args,
        trace_filter=tuple(known.trace_filter or ()),
        policy_overrides=policy,
        pytest_args=pytest_args,
        unittest_args=unittest_args,
        no_framework_filters=known.no_framework_filters,
        autoformat=autoformat_flag,
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


def _run_target_without_recording(config: RecorderCLIConfig) -> int:
    """Execute the target script / test runner without starting the recorder.

    Used when ``CODETRACER_PYTHON_RECORDER_DISABLED=1`` so callers can
    keep the recorder wrapper in their command without paying for trace
    capture.
    """
    sys.stderr.write(
        f"codetracer-python-recorder: recording disabled via {ENV_DISABLED}; "
        "running target without trace capture.\n"
    )
    old_argv = sys.argv
    if config.script:
        sys.argv = [str(config.script)] + config.script_args
    elif config.pytest_args is not None:
        sys.argv = ["pytest"] + config.pytest_args
    elif config.unittest_args is not None:
        sys.argv = ["unittest"] + config.unittest_args
    try:
        if config.pytest_args is not None:
            return _run_pytest(config.pytest_args)
        if config.unittest_args is not None:
            return _run_unittest(config.unittest_args)
        if config.script:
            try:
                runpy.run_path(str(config.script), run_name="__main__")
            except SystemExit as exc:
                return exc.code if isinstance(exc.code, int) else 1
            return 0
        sys.stderr.write("No execution mode specified\n")
        return 1
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

    # P6.2: when ``--no-autoformat`` is on, surface the disable to the
    # runtime via the shared ``CT_AUTOFORMAT`` env var the autoformat
    # module reads.  We set the env var rather than threading a new
    # session option because:
    #   1. The Rust runtime's ``autoformat_enabled_by_env`` already
    #      respects this var (the JS recorder follows the same pattern).
    #   2. ``CT_AUTOFORMAT`` is the cross-recorder kill switch — a
    #      deployment can disable autoformat for Python, JS, and the
    #      replay-server's lazy fallback with one knob.
    # The flag's default is True so the env var stays untouched in the
    # common case — a deployment that sets ``CT_AUTOFORMAT=0`` globally
    # keeps working when users invoke the CLI without flags.  Only
    # explicit ``--no-autoformat`` actively flips the env var.
    # Setting this BEFORE the recording-disabled short-circuit is
    # deliberate so the user-facing kill switch composes correctly
    # with the env-var disable (the flag wins because it's more
    # specific to this invocation).
    if not config.autoformat:
        os.environ["CT_AUTOFORMAT"] = "0"

    # Per convention §5, CODETRACER_PYTHON_RECORDER_DISABLED short-
    # circuits to the un-instrumented target run.  This is the
    # recommended escape hatch for CI pipelines that want to A/B test
    # recorded vs unrecorded execution without changing the command
    # line, mirroring the JS recorder's CODETRACER_JS_RECORDER_DISABLED.
    if recording_disabled():
        return _run_target_without_recording(config)

    trace_dir = config.out_dir
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


__all__ = (
    "ENV_DISABLED",
    "ENV_OUT_DIR",
    "main",
    "recording_disabled",
    "resolve_out_dir",
    "RecorderCLIConfig",
)
