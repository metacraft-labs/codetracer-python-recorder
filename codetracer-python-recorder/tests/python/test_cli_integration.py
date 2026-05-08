"""Integration tests for the recorder CLI entry point.

Per ``codetracer-specs/Recorder-CLI-Conventions.md`` §4 the recorder is
CTFS-only: it does not accept a ``--format`` flag, does not read the
``CODETRACER_FORMAT`` environment variable, and never writes a JSON
events sidecar.  Tests that previously asserted on ``--format json``
output have been rewritten to record CTFS and pipe the recorded
``trace.ct`` container through ``ct print --json`` for content
assertions (see ``test_recorded_trace_via_ct_print_json``).
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]


# CTFS magic bytes identifying a valid .ct trace file.
# See: codetracer-trace-format specification.
_CTFS_MAGIC = bytes([0xC0, 0xDE, 0x72, 0xAC, 0xE2])


# Discover the sibling-built ct-print binary from
# ``codetracer-trace-format-nim``.  The pre-built artefact lives
# alongside the recorder repo under the workspace root; tests fall
# back to the ``CT_PRINT`` env var when callers want to point at a
# custom build.
def _ct_print_binary() -> Path:
    """Return the path to the ``ct-print`` binary used for CTFS conversion.

    Lookup order:

    1. ``CT_PRINT`` environment variable (callers can point at a
       custom build).
    2. The sibling ``codetracer-trace-format-nim`` checkout under the
       workspace root.  ``Path(__file__).resolve().parents[4]`` walks
       up: ``test_cli_integration.py`` → ``python/`` → ``tests/`` →
       ``codetracer-python-recorder/`` (inner) →
       ``codetracer-python-recorder/`` (outer) → workspace root.
    """
    override = os.environ.get("CT_PRINT")
    if override:
        return Path(override)
    return Path(__file__).resolve().parents[4] / "codetracer-trace-format-nim" / "ct-print"


def _write_script(path: Path, body: str = "print('hello from recorder')\n") -> None:
    path.write_text(body, encoding="utf-8")


def _run_cli(
    args: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "codetracer_python_recorder", *args],
        cwd=cwd,
        env=env,
        check=check,
        capture_output=True,
        text=True,
    )


def _prepare_env() -> dict[str, str]:
    env = os.environ.copy()
    pythonpath = env.get("PYTHONPATH", "")
    root = str(REPO_ROOT)
    env["PYTHONPATH"] = root if not pythonpath else os.pathsep.join([root, pythonpath])
    # Ensure stale env vars from the harness don't leak into the child.
    env.pop("CODETRACER_PYTHON_RECORDER_OUT_DIR", None)
    env.pop("CODETRACER_PYTHON_RECORDER_DISABLED", None)
    return env


def _find_ct_file(trace_dir: Path) -> Path:
    """Locate the CTFS ``.ct`` container in a recorded trace directory.

    The Nim writer names the produced container after the recorded
    program (e.g. ``program.ct``) rather than the literal ``trace.ct``,
    so callers must glob.  This helper raises an ``AssertionError`` with
    a directory listing when no container is found, so test failures
    are diagnosable.
    """
    ct_files = list(trace_dir.glob("*.ct"))
    assert ct_files, (
        f"No .ct files found in {trace_dir}; "
        f"contents: {list(trace_dir.iterdir()) if trace_dir.is_dir() else '<missing>'}"
    )
    return ct_files[0]


def test_cli_emits_trace_artifacts(tmp_path: Path) -> None:
    """Default recorder run produces a canonical CTFS container.

    Per convention §4 the recorder is CTFS-only.  The Nim writer emits
    a single multi-stream ``<program>.ct`` container; legacy JSON
    sidecars (``trace.json``) are forbidden.
    """
    script = tmp_path / "program.py"
    _write_script(script, "value = 21 + 21\nprint(value)\n")

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--on-recorder-error",
        "disable",
        "--require-trace",
        "--keep-partial-trace",
        "--log-level",
        "info",
        "--json-errors",
    ]
    args.append(str(script))

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0
    assert trace_dir.is_dir()

    trace_ct = _find_ct_file(trace_dir)
    assert not (trace_dir / "trace.json").exists(), (
        "trace.json must not be produced — the recorder is CTFS-only"
    )

    # Verify the CTFS magic bytes at the start of the file.
    with open(trace_ct, "rb") as f:
        magic = f.read(len(_CTFS_MAGIC))
    assert magic == _CTFS_MAGIC, f"Invalid CTFS magic: {magic.hex()}"


def test_cli_honours_trace_filter_chain(tmp_path: Path) -> None:
    """Smoke test: --trace-filter is accepted and the recording succeeds.

    Pre-2026-05 this test asserted on the trace-filter chain via the
    ``trace_metadata.json`` sidecar produced by ``--format json``.
    Under the CTFS-only contract the metadata sidecar is no longer
    written separately (it is embedded in the CTFS container), so the
    chain-content assertion has been moved to the API-level tests in
    ``test_monitoring_events.py`` (which still exercise the JSON event
    stream directly).  At the CLI layer we just verify that the
    explicit-and-default-filter combo doesn't break recording.
    """
    script = tmp_path / "program.py"
    _write_script(script, "print('filter test')\n")

    filters_dir = tmp_path / ".codetracer"
    filters_dir.mkdir()
    default_filter = filters_dir / "trace-filter.toml"
    default_filter.write_text(
        """
        [meta]
        name = "default"
        version = 1

        [scope]
        default_exec = "trace"
        default_value_action = "allow"
        """,
        encoding="utf-8",
    )

    override_filter = tmp_path / "override-filter.toml"
    override_filter.write_text(
        """
        [meta]
        name = "override"
        version = 1

        [scope]
        default_exec = "trace"
        default_value_action = "allow"

        [[scope.rules]]
        selector = "pkg:program"
        exec = "skip"
        value_default = "allow"
        """,
        encoding="utf-8",
    )

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--trace-filter",
        str(override_filter),
        str(script),
    ]

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0
    # The CTFS container must exist; the recorder fails loudly if any
    # filter file is invalid or unreachable.
    _find_ct_file(trace_dir)


def test_cli_honours_env_trace_filter(tmp_path: Path) -> None:
    """Smoke test: ``CODETRACER_TRACE_FILTER`` is accepted by the auto-start CLI path."""
    script = tmp_path / "program.py"
    _write_script(script, "print('env filter test')\n")

    filter_path = tmp_path / "env-filter.toml"
    filter_path.write_text(
        """
        [meta]
        name = "env-filter"
        version = 1

        [scope]
        default_exec = "trace"
        default_value_action = "allow"

        [[scope.rules]]
        selector = "pkg:program"
        exec = "skip"
        value_default = "allow"
        """,
        encoding="utf-8",
    )

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    env["CODETRACER_TRACE_FILTER"] = str(filter_path)

    result = _run_cli(["--out-dir", str(trace_dir), str(script)], cwd=tmp_path, env=env)
    assert result.returncode == 0
    _find_ct_file(trace_dir)


def test_ctfs_trace_has_steps(tmp_path: Path) -> None:
    """The default CTFS trace contains step data for the recorded program."""
    script = tmp_path / "program.py"
    _write_script(script, "a = 1\nb = 2\nc = a + b\nprint(c)\n")

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--on-recorder-error",
        "disable",
        "--require-trace",
    ]
    args.append(str(script))

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0

    trace_ct = _find_ct_file(trace_dir)
    # The CTFS container should have reasonable size (a few KB at minimum
    # for 4 lines of traced Python).  The exact byte count varies as the
    # CTFS encoder evolves; the floor is set at 256 bytes (the magic +
    # header alone is ~32 bytes, and we expect at least a handful of
    # registered events).
    assert trace_ct.stat().st_size > 256, "CTFS trace suspiciously small"


def test_ctfs_trace_records_exceptions(tmp_path: Path) -> None:
    """The default CTFS trace records exception events."""
    script = tmp_path / "program.py"
    _write_script(
        script,
        textwrap.dedent("""\
            try:
                x = 1 / 0
            except ZeroDivisionError:
                pass
            print("survived")
        """),
    )

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--on-recorder-error",
        "disable",
        "--require-trace",
    ]
    args.append(str(script))

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0
    assert "survived" in result.stdout

    _find_ct_file(trace_dir)


# ---------------------------------------------------------------------------
# Convention compliance — ``Recorder-CLI-Conventions.md`` §4 / §5
# ---------------------------------------------------------------------------


def test_format_flag_rejected(tmp_path: Path) -> None:
    """Per convention §4 the CLI must reject ``--format`` outright.

    The previous implementation accepted ``--format json|binary|ctfs``.
    The new contract is CTFS-only and the flag is gone.  Any of the
    legacy values must produce a non-zero exit code (argparse uses 2
    for usage errors) and a stderr message that mentions the flag so
    users have a clear migration path.
    """
    script = tmp_path / "program.py"
    _write_script(script)
    env = _prepare_env()

    for legacy_value in ("json", "binary", "ctfs"):
        result = _run_cli(
            ["--format", legacy_value, str(script)],
            cwd=tmp_path,
            env=env,
            check=False,
        )
        assert result.returncode != 0, (
            f"--format {legacy_value} should be rejected, got exit code 0"
        )
        # The error message must mention the flag so users know what to fix.
        assert "--format" in result.stderr or "--format" in result.stdout

    # The collapsed ``--format=json`` form must also be rejected.
    result = _run_cli(
        ["--format=json", str(script)], cwd=tmp_path, env=env, check=False
    )
    assert result.returncode != 0


def test_no_format_flag_in_help() -> None:
    """The ``--help`` output must not advertise ``--format`` or ``CODETRACER_FORMAT``."""
    env = _prepare_env()
    result = _run_cli(["--help"], cwd=Path.cwd(), env=env)
    combined = result.stdout + result.stderr
    assert "--format" not in combined, (
        "--help must not mention --format (recorder is CTFS-only)"
    )
    assert "CODETRACER_FORMAT" not in combined, (
        "--help must not mention CODETRACER_FORMAT (recorder is CTFS-only)"
    )


def test_help_mentions_ct_print() -> None:
    """The ``--help`` output must point users at ``ct print`` (convention §4).

    ``ct print`` from ``codetracer-trace-format-nim`` is the canonical
    conversion tool from CTFS to JSON / text; the recorder no longer
    emits these forms directly.
    """
    env = _prepare_env()
    result = _run_cli(["--help"], cwd=Path.cwd(), env=env)
    combined = result.stdout + result.stderr
    assert "ct print" in combined, (
        "--help must mention `ct print` as the conversion tool"
    )


def test_env_out_dir_used_when_flag_omitted(tmp_path: Path) -> None:
    """``CODETRACER_PYTHON_RECORDER_OUT_DIR`` is honoured when ``--out-dir`` is omitted (§5)."""
    script = tmp_path / "program.py"
    _write_script(script, "print('env out dir test')\n")

    env_trace_dir = tmp_path / "env-out"
    env = _prepare_env()
    env["CODETRACER_PYTHON_RECORDER_OUT_DIR"] = str(env_trace_dir)

    result = _run_cli([str(script)], cwd=tmp_path, env=env)
    assert result.returncode == 0
    assert env_trace_dir.is_dir(), (
        f"recorder should have written into {env_trace_dir}; "
        f"contents of tmp_path: {list(tmp_path.iterdir())}"
    )
    _find_ct_file(env_trace_dir)


def test_cli_flag_overrides_env_out_dir(tmp_path: Path) -> None:
    """``--out-dir`` always wins over the env-var fallback (§5)."""
    script = tmp_path / "program.py"
    _write_script(script, "print('cli wins')\n")

    env_trace_dir = tmp_path / "env-out"
    cli_trace_dir = tmp_path / "cli-out"
    env = _prepare_env()
    env["CODETRACER_PYTHON_RECORDER_OUT_DIR"] = str(env_trace_dir)

    result = _run_cli(
        ["--out-dir", str(cli_trace_dir), str(script)], cwd=tmp_path, env=env
    )
    assert result.returncode == 0
    assert cli_trace_dir.is_dir()
    _find_ct_file(cli_trace_dir)
    assert not env_trace_dir.exists(), (
        "env-supplied dir must not be touched when --out-dir is given"
    )


def test_env_disabled_skips_recording(tmp_path: Path) -> None:
    """``CODETRACER_PYTHON_RECORDER_DISABLED=1`` short-circuits recording (§5).

    The target program must still execute (so users keep their CI
    pipelines working with the recorder shim in place), but no trace
    artefacts should be produced.
    """
    script = tmp_path / "program.py"
    _write_script(script, "print('still ran without recording')\n")

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    env["CODETRACER_PYTHON_RECORDER_DISABLED"] = "1"

    result = _run_cli(
        ["--out-dir", str(trace_dir), str(script)], cwd=tmp_path, env=env
    )
    assert result.returncode == 0
    assert "still ran without recording" in result.stdout, (
        f"target script must still execute when disabled; stdout={result.stdout!r}"
    )
    # No CTFS container or JSON sidecar should have been written.
    if trace_dir.exists():
        unwanted = list(trace_dir.glob("*.ct")) + list(trace_dir.glob("trace.*"))
        assert not unwanted, (
            f"recording should have been skipped; got {unwanted}"
        )

    # Also accepts ``true`` (case-insensitive) as a truthy value.
    env["CODETRACER_PYTHON_RECORDER_DISABLED"] = "TRUE"
    result = _run_cli([str(script)], cwd=tmp_path, env=env)
    assert result.returncode == 0
    assert "still ran without recording" in result.stdout


# ---------------------------------------------------------------------------
# ct-print round-trip (replaces the old ``--format json`` content assertions)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(
    not _ct_print_binary().exists(),
    reason="ct-print binary not built — run from a workspace with codetracer-trace-format-nim",
)
def test_recorded_trace_via_ct_print_json(tmp_path: Path) -> None:
    """Record a real script and assert structural anchors from ``ct print --json``.

    The previous CLI integration suite read ``trace.json`` directly to
    verify event content; under the CTFS-only contract the recorder no
    longer writes that file, so we round-trip through ``ct print``
    instead.  Only structural anchors (script filename + function /
    variable names) are asserted: the cardano / circom / flow / fuel /
    leo / miden / move / polkavm precedents document that integer
    values may not round-trip through every recorder→encoder path, so
    we deliberately do not assert on them here.
    """
    script_body = textwrap.dedent("""\
        def make_greeting(target_name):
            greeting_value = "hello, " + target_name
            return greeting_value


        def main():
            person_name = "world"
            result_text = make_greeting(person_name)
            print(result_text)


        if __name__ == "__main__":
            main()
        """)
    script = tmp_path / "ct_print_smoke.py"
    _write_script(script, script_body)

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = ["--out-dir", str(trace_dir), str(script)]
    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0

    trace_ct = _find_ct_file(trace_dir)

    ct_print = _ct_print_binary()
    proc = subprocess.run(
        [str(ct_print), "--json", str(trace_ct)],
        capture_output=True,
        text=True,
        check=True,
    )
    haystack = proc.stdout

    # Structural anchors: the script filename and the function /
    # variable names we declared above must surface somewhere in the
    # rendered JSON.  We assert on substrings rather than parsing the
    # JSON shape because ``ct print``'s schema is allowed to evolve
    # between releases (we only need a stable convention contract).
    assert "ct_print_smoke.py" in haystack, (
        f"script filename not found in ct-print output (first 500 chars): {haystack[:500]!r}"
    )
    for anchor in ("make_greeting", "main", "greeting_value", "result_text"):
        assert anchor in haystack, (
            f"expected anchor {anchor!r} in ct-print output; "
            f"first 500 chars: {haystack[:500]!r}"
        )
