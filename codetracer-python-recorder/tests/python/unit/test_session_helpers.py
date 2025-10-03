"""Unit tests for session helper functions."""
from __future__ import annotations

from pathlib import Path

import pytest

from codetracer_python_recorder import session


def test_coerce_format_accepts_supported_aliases() -> None:
    assert session._coerce_format("json") == "json"
    assert session._coerce_format("JSON") == "json"
    assert session._coerce_format("binary") == "binary"


def test_coerce_format_rejects_unknown_value() -> None:
    with pytest.raises(ValueError) as excinfo:
        session._coerce_format("yaml")
    assert "unsupported trace format" in str(excinfo.value)


def test_validate_trace_path_expands_user(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    home_dir = tmp_path / "home"
    home_dir.mkdir()
    target = home_dir / "traces"

    monkeypatch.setenv("HOME", str(home_dir))
    path = session._validate_trace_path(Path("~/traces"))

    assert path == target
    assert path.parent == home_dir


def test_validate_trace_path_rejects_file(tmp_path: Path) -> None:
    file_path = tmp_path / "trace.bin"
    file_path.write_text("stub")
    with pytest.raises(ValueError):
        session._validate_trace_path(file_path)


def test_normalize_activation_path_handles_none() -> None:
    assert session._normalize_activation_path(None) is None


def test_normalize_activation_path_expands_user(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    home_dir = tmp_path / "home"
    home_dir.mkdir()
    script = home_dir / "script.py"
    script.write_text("print('hi')\n")

    monkeypatch.setenv("HOME", str(home_dir))
    result = session._normalize_activation_path("~/script.py")

    assert result == str(script)


@pytest.fixture(autouse=True)
def clear_active_session() -> None:
    session._active_session = None
    yield
    session._active_session = None


def test_trace_session_stop_clears_global(monkeypatch: pytest.MonkeyPatch) -> None:
    called = {"stop": False, "start": False}

    def fake_start(*args, **kwargs) -> None:
        called["start"] = True

    def fake_stop() -> None:
        called["stop"] = True

    monkeypatch.setattr(session, "_start_backend", fake_start)
    monkeypatch.setattr(session, "_stop_backend", fake_stop)
    monkeypatch.setattr(session, "_is_tracing_backend", lambda: session._active_session is not None)

    session._active_session = session.TraceSession(path=Path("/tmp"), format="json")
    session.stop()
    assert session._active_session is None
    assert called["stop"] is True


def test_trace_session_flush_noop_when_inactive(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(session, "_is_tracing_backend", lambda: False)
    flushed = []

    def fake_flush() -> None:
        flushed.append(True)

    monkeypatch.setattr(session, "_flush_backend", fake_flush)
    session.flush()
    assert flushed == []


def test_trace_context_manager_starts_and_stops(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    calls = {"start": [], "stop": []}

    trace_state = {"active": False}

    def fake_start(path: str, fmt: str, activation: str | None) -> None:
        trace_state["active"] = True
        calls["start"].append((Path(path), fmt, activation))

    def fake_stop() -> None:
        trace_state["active"] = False
        calls["stop"].append(True)

    monkeypatch.setattr(session, "_start_backend", fake_start)
    monkeypatch.setattr(session, "_stop_backend", fake_stop)
    monkeypatch.setattr(session, "_is_tracing_backend", lambda: trace_state["active"])
    monkeypatch.setattr(session, "_flush_backend", lambda: None)

    target = tmp_path / "trace"
    target.mkdir()
    with session.trace(target, format="binary") as handle:
        assert isinstance(handle, session.TraceSession)
        assert handle.path == target.expanduser()
        assert handle.format == "binary"

    assert calls["start"] == [(target, "binary", None)]
    assert calls["stop"] == [True]
