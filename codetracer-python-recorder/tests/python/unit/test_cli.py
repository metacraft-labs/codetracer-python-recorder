"""Unit tests for the recorder CLI helpers."""
from __future__ import annotations

from pathlib import Path

import pytest

from codetracer_python_recorder import formats
from codetracer_python_recorder.cli import _parse_args


def _write_script(path: Path) -> None:
    path.write_text("print('cli test')\n", encoding="utf-8")


def test_parse_args_uses_defaults(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    script = Path("sample.py")
    _write_script(script)

    config = _parse_args([str(script)])

    assert config.script == script.resolve()
    assert config.script_args == []
    assert config.trace_dir == (tmp_path / "trace-out").resolve()
    assert config.format == formats.DEFAULT_FORMAT
    assert config.activation_path == script.resolve()
    assert config.trace_filter == ()
    assert config.policy_overrides == {}


def test_parse_args_accepts_custom_trace_dir(tmp_path: Path) -> None:
    script = tmp_path / "app.py"
    _write_script(script)
    trace_dir = tmp_path / "custom-trace"

    config = _parse_args(["--trace-dir", str(trace_dir), str(script)])

    assert config.trace_dir == trace_dir.resolve()


def test_parse_args_validates_format(tmp_path: Path) -> None:
    script = tmp_path / "main.py"
    _write_script(script)

    with pytest.raises(SystemExit):
        _parse_args(["--format", "yaml", str(script)])


def test_parse_args_handles_activation_and_script_args(tmp_path: Path) -> None:
    script = tmp_path / "prog.py"
    _write_script(script)
    activation = tmp_path / "activation.py"
    _write_script(activation)

    config = _parse_args(
        [
            "--activation-path",
            str(activation),
            str(script),
            "--",
            "--flag",
            "value",
        ]
    )

    assert config.activation_path == activation.resolve()
    assert config.script_args == ["--flag", "value"]
    assert config.trace_filter == ()
    assert config.policy_overrides == {}


def test_parse_args_collects_policy_overrides(tmp_path: Path) -> None:
    script = tmp_path / "entry.py"
    _write_script(script)
    log_file = tmp_path / "logs" / "recorder.log"

    config = _parse_args(
        [
            "--on-recorder-error",
            "disable",
            "--require-trace",
            "--keep-partial-trace",
            "--log-level",
            "debug",
            "--log-file",
            str(log_file),
            "--json-errors",
            str(script),
        ]
    )

    assert config.policy_overrides == {
        "on_recorder_error": "disable",
        "require_trace": True,
        "keep_partial_trace": True,
        "log_level": "debug",
        "log_file": (tmp_path / "logs" / "recorder.log").resolve(),
        "json_errors": True,
    }


def test_parse_args_controls_io_capture(tmp_path: Path) -> None:
    script = tmp_path / "entry.py"
    _write_script(script)

    config = _parse_args(
        [
            "--io-capture",
            "off",
            str(script),
        ]
    )

    assert config.policy_overrides == {
        "io_capture_line_proxies": False,
        "io_capture_fd_fallback": False,
    }


def test_parse_args_collects_trace_filters(tmp_path: Path) -> None:
    script = tmp_path / "app.py"
    _write_script(script)
    filter_a = tmp_path / "filters" / "default.toml"
    filter_a.parent.mkdir(parents=True, exist_ok=True)
    filter_a.write_text("# stub\n", encoding="utf-8")
    filter_b = tmp_path / "filters" / "override.toml"
    filter_b.write_text("# stub\n", encoding="utf-8")

    config = _parse_args(
        [
            "--trace-filter",
            str(filter_a),
            "--trace-filter",
            f"{filter_b}::{filter_a}",
            str(script),
        ]
    )

    assert config.trace_filter == (str(filter_a), f"{filter_b}::{filter_a}")


def test_parse_args_enables_io_capture_fd_mirroring(tmp_path: Path) -> None:
    script = tmp_path / "entry.py"
    _write_script(script)

    config = _parse_args(
        [
            "--io-capture",
            "proxies+fd",
            str(script),
        ]
    )

    assert config.policy_overrides == {
        "io_capture_line_proxies": True,
        "io_capture_fd_fallback": True,
    }


def test_parse_args_enables_module_name_from_globals(tmp_path: Path) -> None:
    script = tmp_path / "entry.py"
    _write_script(script)

    config = _parse_args(
        [
            "--module-name-from-globals",
            str(script),
        ]
    )

    assert config.policy_overrides == {
        "module_name_from_globals": True,
    }
