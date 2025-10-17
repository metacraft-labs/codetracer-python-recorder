"""Integration tests for the recorder CLI entry point."""
from __future__ import annotations

import json
import os
import subprocess
import sys
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
        "--trace-dir",
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
        "--trace-dir",
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

    result = _run_cli(["--trace-dir", str(trace_dir), str(script)], cwd=tmp_path, env=env)
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
