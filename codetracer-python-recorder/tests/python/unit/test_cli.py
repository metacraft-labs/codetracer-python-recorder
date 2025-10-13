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
