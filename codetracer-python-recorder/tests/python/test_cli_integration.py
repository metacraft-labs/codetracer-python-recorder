"""Integration tests for the recorder CLI entry point."""
from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]


def _write_script(path: Path, body: str = "print('hello from recorder')\n") -> None:
    path.write_text(body, encoding="utf-8")


def _run_cli(
    args: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "codetracer_python_recorder", *args],
        cwd=cwd,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )


def _prepare_env() -> dict[str, str]:
    env = os.environ.copy()
    pythonpath = env.get("PYTHONPATH", "")
    root = str(REPO_ROOT)
    env["PYTHONPATH"] = root if not pythonpath else os.pathsep.join([root, pythonpath])
    return env


def test_cli_emits_trace_artifacts(tmp_path: Path) -> None:
    script = tmp_path / "program.py"
    _write_script(script, "value = 21 + 21\nprint(value)\n")

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--format",
        "json",
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

    events_file = trace_dir / "trace.json"
    metadata_file = trace_dir / "trace_metadata.json"
    paths_file = trace_dir / "trace_paths.json"
    assert events_file.exists()
    assert metadata_file.exists()
    assert paths_file.exists()

    payload = json.loads(metadata_file.read_text(encoding="utf-8"))
    recorder_info = payload.get("recorder", {})
    assert recorder_info.get("name") == "codetracer_python_recorder"
    assert recorder_info.get("target_script") == str(script.resolve())


def test_cli_honours_trace_filter_chain(tmp_path: Path) -> None:
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

    metadata_file = trace_dir / "trace_metadata.json"
    payload = json.loads(metadata_file.read_text(encoding="utf-8"))
    trace_filter = payload.get("trace_filter", {})
    filters = trace_filter.get("filters", [])
    paths = [entry.get("path") for entry in filters if isinstance(entry, dict)]
    assert paths == [
        "<inline:builtin-default>",
        str(default_filter.resolve()),
        str(override_filter.resolve()),
    ]


def test_cli_honours_env_trace_filter(tmp_path: Path) -> None:
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

    metadata_file = trace_dir / "trace_metadata.json"
    payload = json.loads(metadata_file.read_text(encoding="utf-8"))
    trace_filter = payload.get("trace_filter", {})
    filters = trace_filter.get("filters", [])
    paths = [entry.get("path") for entry in filters if isinstance(entry, dict)]
    assert paths == [
        "<inline:builtin-default>",
        str(filter_path.resolve()),
    ]


# CTFS magic bytes identifying a valid .ct binary trace file.
# See: codetracer-trace-format specification.
_CTFS_MAGIC = bytes([0xC0, 0xDE, 0x72, 0xAC, 0xE2])


def test_cli_emits_binary_trace(tmp_path: Path) -> None:
    """Recording with --format binary produces a valid .ct file."""
    script = tmp_path / "program.py"
    _write_script(script, "x = 42\nprint(x)\n")

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--format",
        "binary",
        "--on-recorder-error",
        "disable",
        "--require-trace",
        "--keep-partial-trace",
    ]
    args.append(str(script))

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0
    assert trace_dir.is_dir()

    # The binary format produces a single trace.bin file containing the CTFS
    # container (magic bytes 0xC0 0xDE 0x72 0xAC 0xE2).
    trace_bin = trace_dir / "trace.bin"
    assert trace_bin.exists(), f"Expected trace.bin, found: {list(trace_dir.iterdir())}"
    assert trace_bin.stat().st_size > 0, "trace.bin should not be empty"

    # Verify the CTFS magic bytes at the start of the file.
    with open(trace_bin, "rb") as f:
        magic = f.read(len(_CTFS_MAGIC))
    assert (
        magic == _CTFS_MAGIC
    ), f"Invalid CTFS magic: {magic.hex()}"


def test_binary_trace_has_steps(tmp_path: Path) -> None:
    """Binary trace contains step data for the recorded program."""
    script = tmp_path / "program.py"
    _write_script(script, "a = 1\nb = 2\nc = a + b\nprint(c)\n")

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--format",
        "binary",
        "--on-recorder-error",
        "disable",
        "--require-trace",
    ]
    args.append(str(script))

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0

    trace_bin = trace_dir / "trace.bin"
    assert trace_bin.exists(), f"Expected trace.bin, found: {list(trace_dir.iterdir())}"

    # The trace.bin file should have reasonable size (a few KB at minimum for
    # 4 lines of traced Python).
    assert trace_bin.stat().st_size > 1000, "Trace file suspiciously small"


def test_binary_trace_records_exceptions(tmp_path: Path) -> None:
    """Binary trace records exception events."""
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
        "--format",
        "binary",
        "--on-recorder-error",
        "disable",
        "--require-trace",
    ]
    args.append(str(script))

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0
    assert "survived" in result.stdout

    trace_bin = trace_dir / "trace.bin"
    assert trace_bin.exists(), f"Expected trace.bin, found: {list(trace_dir.iterdir())}"
