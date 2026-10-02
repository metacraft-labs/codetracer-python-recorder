from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest


def _run_cli(script: Path, *args: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    cmd = [
        sys.executable,
        "-m",
        "codetracer_python_recorder",
        *args,
        str(script),
    ]
    return subprocess.run(cmd, capture_output=True, text=True, env=env, check=False)


def _assert_injection_support(result: subprocess.CompletedProcess[str]) -> None:
    """Assert that error injection is supported (integration-test feature enabled).

    If injection is not working, the test fails with instructions on how to fix.
    Run `just dev` to build with the integration-test feature enabled.
    """
    if result.returncode == 0 and "test-injected failure" not in result.stderr:
        pytest.fail(
            "Recorder built without integration-test hooks.\n"
            "Run `just dev` (or `just test` which includes it) to build with "
            "the integration-test feature enabled."
        )


def test_cli_disable_policy_detaches_on_internal_error(tmp_path: Path) -> None:
    script = tmp_path / "app.py"
    script.write_text("value = 1\nprint(value)\n")
    trace_dir = tmp_path / "trace"

    env = os.environ.copy()
    env["CODETRACER_TEST_INJECT_FAILURE"] = "line"

    result = _run_cli(
        script,
        "--out-dir",
        str(trace_dir),
        "--on-recorder-error",
        "disable",
        env=env,
    )

    _assert_injection_support(result)

    assert result.returncode == 0, result.stderr
    assert trace_dir.is_dir()
    # The recorder names its container after the program (``app.ct``); the
    # disable-policy path must clean every artefact away.
    legacy_events = trace_dir / "trace.json"
    metadata = trace_dir / "trace_metadata.json"
    paths = trace_dir / "trace_paths.json"
    assert not list(trace_dir.glob("*.ct")), (
        f"the failed recording's container was left behind: {sorted(trace_dir.iterdir())}"
    )
    assert not legacy_events.exists(), (
        "trace.json must never be written (CTFS-only)"
    )
    assert not metadata.exists()
    assert not paths.exists()
    assert "test-injected failure" in result.stderr


def test_cli_abort_policy_propagates_internal_error(tmp_path: Path) -> None:
    script = tmp_path / "app.py"
    script.write_text("value = 1\nprint(value)\n")
    trace_dir = tmp_path / "trace"

    env = os.environ.copy()
    env["CODETRACER_TEST_INJECT_FAILURE"] = "line"

    result = _run_cli(
        script,
        "--out-dir",
        str(trace_dir),
        "--on-recorder-error",
        "abort",
        env=env,
    )

    _assert_injection_support(result)

    assert result.returncode != 0
    assert trace_dir.is_dir()
    assert "test-injected failure" in result.stderr


@pytest.mark.skipif(not hasattr(os, "symlink"), reason="symlinks required")
def test_cli_require_trace_fails_when_no_events_recorded(tmp_path: Path) -> None:
    script = tmp_path / "real_script.py"
    script.write_text("print('ran')\n")
    alias = tmp_path / "alias.py"
    try:
        alias.symlink_to(script)
    except OSError as exc:  # pragma: no cover - platform dependent branch
        pytest.skip(f"symlinks unavailable: {exc}")

    trace_dir = tmp_path / "trace"
    env = os.environ.copy()
    env["CODETRACER_TEST_INJECT_FAILURE"] = "suppress-events"

    result = _run_cli(
        alias,
        "--out-dir",
        str(trace_dir),
        "--require-trace",
        env=env,
    )

    # Check injection support - a container on success means it's not working.
    if result.returncode == 0 and list(trace_dir.glob("*.ct")):
        pytest.fail(
            "Recorder built without integration-test hooks.\n"
            "Run `just dev` (or `just test` which includes it) to build with "
            "the integration-test feature enabled."
        )

    assert result.returncode != 0
    assert trace_dir.is_dir()
    assert "requires a trace but no events were recorded" in result.stderr


def test_cli_json_errors_emits_trailer(tmp_path: Path) -> None:
    script = tmp_path / "app.py"
    script.write_text("value = 1\nprint(value)\n")
    trace_dir = tmp_path / "trace"

    env = os.environ.copy()
    env["CODETRACER_TEST_INJECT_FAILURE"] = "line"

    result = _run_cli(
        script,
        "--out-dir",
        str(trace_dir),
        "--json-errors",
        "--on-recorder-error",
        "abort",
        env=env,
    )

    _assert_injection_support(result)

    assert result.returncode != 0
    lines = [line for line in result.stderr.splitlines() if line.strip()]
    for line in reversed(lines):
        if line.lstrip().startswith("{"):
            trailer = json.loads(line)
            break
    else:
        raise AssertionError(f"missing JSON error trailer in stderr: {result.stderr!r}")
    assert trailer["error_code"] == "ERR_TRACE_INCOMPLETE"
    assert trailer["message"].startswith("test-injected failure")
    assert trailer["run_id"]
    assert trailer["trace_id"]
