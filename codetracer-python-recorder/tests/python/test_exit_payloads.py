from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

from codetracer_python_recorder.trace_balance import load_trace_events


def _last_return_value(trace_dir: Path) -> dict[str, object]:
    events = load_trace_events(trace_dir / "trace.json")
    for event in reversed(events):
        payload = event.get("Return")
        if payload is not None:
            return payload["return_value"]
    raise AssertionError("trace did not contain any Return events")


def test_cli_records_exit_code_in_toplevel_return(tmp_path: Path) -> None:
    script = tmp_path / "exit_script.py"
    script.write_text(
        "import sys\n"
        "sys.exit(3)\n",
        encoding="utf-8",
    )

    trace_dir = tmp_path / "trace"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "codetracer_python_recorder",
            "--trace-dir",
            str(trace_dir),
            "--format",
            "json",
            str(script),
        ],
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert "status 3; returning 0" in result.stderr
    exit_value = _last_return_value(trace_dir)
    assert exit_value["kind"] == "Int"
    assert exit_value["i"] == 3

    metadata = json.loads((trace_dir / "trace_metadata.json").read_text(encoding="utf-8"))
    status = metadata.get("process_exit_status")
    assert status == {"code": 3, "label": None}


def test_cli_can_propagate_script_exit(tmp_path: Path) -> None:
    script = tmp_path / "exit_script.py"
    script.write_text(
        "import sys\n"
        "sys.exit(5)\n",
        encoding="utf-8",
    )

    trace_dir = tmp_path / "trace"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "codetracer_python_recorder",
            "--trace-dir",
            str(trace_dir),
            "--format",
            "json",
            "--propagate-script-exit",
            str(script),
        ],
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 5, result.stderr
    assert "status 5; returning 0" not in result.stderr
    exit_value = _last_return_value(trace_dir)
    assert exit_value["kind"] == "Int"
    assert exit_value["i"] == 5

    metadata = json.loads((trace_dir / "trace_metadata.json").read_text(encoding="utf-8"))
    status = metadata.get("process_exit_status")
    assert status == {"code": 5, "label": None}


def test_default_exit_payload_uses_placeholder(tmp_path: Path) -> None:
    trace_dir = tmp_path / "trace"
    trace_dir.mkdir()

    # Directly call the start/stop API without providing an exit code.
    script = (
        "import json\n"
        "from pathlib import Path\n"
        "import codetracer_python_recorder as codetracer\n"
        f"trace_dir = Path({json.dumps(str(trace_dir))!s})\n"
        "session = codetracer.start(trace_dir, format='json')\n"
        "session.stop()\n"
    )
    subprocess.run([sys.executable, "-c", script], check=True)

    exit_value = _last_return_value(trace_dir)
    assert exit_value["kind"] == "String"
    assert exit_value["text"] == "<exit>"

    metadata = json.loads((trace_dir / "trace_metadata.json").read_text(encoding="utf-8"))
    status = metadata.get("process_exit_status")
    assert status == {"code": None, "label": "<exit>"}
