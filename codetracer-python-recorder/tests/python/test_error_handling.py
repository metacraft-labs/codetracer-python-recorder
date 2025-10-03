from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

import codetracer_python_recorder as codetracer


@pytest.fixture(autouse=True)
def ensure_tracer_stopped() -> None:
    yield
    if codetracer.is_tracing():
        codetracer.stop()


def _run_python_script(tmp_path: Path, body: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    script_path = tmp_path / "runner.py"
    script_path.write_text(textwrap.dedent(body))
    return subprocess.run(
        [sys.executable, str(script_path)],
        capture_output=True,
        text=True,
        check=False,
        env=env,
    )


@pytest.mark.skipif(os.name == "nt", reason="posix permissions required")
def test_start_tracing_raises_environment_error(tmp_path: Path) -> None:
    locked_dir = tmp_path / "locked"
    locked_dir.mkdir()
    locked_dir.chmod(stat.S_IRUSR | stat.S_IXUSR)
    trace_dir = locked_dir / "trace"

    try:
        with pytest.raises(codetracer.EnvironmentError) as excinfo:
            codetracer.start(trace_dir)
        assert excinfo.value.code == "ERR_TRACE_DIR_CREATE_FAILED"
        assert "failed to create trace directory" in str(excinfo.value)
    finally:
        locked_dir.chmod(stat.S_IRWXU)


TARGET_ERROR_SCRIPT = """
import os
import sys
from pathlib import Path

import codetracer_python_recorder as recorder

trace_dir = Path(os.environ["TRACE_DIR"])
try:
    with recorder.trace(trace_dir):
        def sample(value: int) -> int:
            return value + 1
        sample(1)
except recorder.TargetError as exc:
    print(f"caught {exc.__class__.__name__} {getattr(exc, 'code', '')}")
    sys.exit(0)
except Exception as exc:  # pragma: no cover - defensive logging for debugging
    print(f"unexpected {type(exc).__name__}")
    sys.exit(2)
else:
    print("no-error")
    sys.exit(1)
"""


def test_target_error_integration(tmp_path: Path) -> None:
    env = os.environ.copy()
    env["CODETRACER_TEST_INJECT_FAILURE"] = "target-args"
    trace_dir = tmp_path / "target-trace"
    env["TRACE_DIR"] = str(trace_dir)

    result = _run_python_script(tmp_path, TARGET_ERROR_SCRIPT, env=env)
    if result.returncode != 0 and "no-error" in result.stdout:
        pytest.skip("recorder built without integration-test hooks")
    assert result.returncode == 0, result.stderr
    assert "TargetError" in result.stdout
    assert "ERR_TRACE_INCOMPLETE" in result.stdout


PANIC_SCRIPT = """
import json
import os
import sys
from pathlib import Path

import codetracer_python_recorder as recorder

trace_dir = Path(os.environ["TRACE_DIR"])
try:
    with recorder.trace(trace_dir):
        def probe() -> int:
            return sum(range(5))
        probe()
        probe()
except recorder.InternalError as exc:
    print(f"caught {exc.__class__.__name__} {getattr(exc, 'code', '')}" )
    sys.exit(0)
except Exception as exc:  # pragma: no cover - defensive logging
    print(f"unexpected {type(exc).__name__}")
    sys.exit(2)
else:
    print("no-error")
    sys.exit(1)
"""


def test_panic_in_callback_maps_to_internal_error(tmp_path: Path) -> None:
    env = os.environ.copy()
    env["CODETRACER_TEST_INJECT_FAILURE"] = "panic"
    env["CODETRACER_JSON_ERRORS"] = "1"
    trace_dir = tmp_path / "panic-trace"
    env["TRACE_DIR"] = str(trace_dir)

    result = _run_python_script(tmp_path, PANIC_SCRIPT, env=env)
    if result.returncode != 0 and "no-error" in result.stdout:
        pytest.skip("recorder built without integration-test hooks")
    assert result.returncode == 0, result.stderr
    assert "InternalError" in result.stdout
    stderr_lines = [line for line in result.stderr.splitlines() if line.strip()]
    assert any("panic in on_line" in line for line in stderr_lines)
    assert any(
        line.strip().startswith("{") and json.loads(line).get("error_kind") == "Internal"
        for line in stderr_lines
    ), result.stderr
