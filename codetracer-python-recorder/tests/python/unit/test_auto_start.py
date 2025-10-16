"""Unit tests for environment-driven auto-start behaviour."""
from __future__ import annotations

from pathlib import Path

import pytest

from codetracer_python_recorder import auto_start, session
import codetracer_python_recorder.codetracer_python_recorder as backend


@pytest.fixture(autouse=True)
def reset_session_state() -> None:
    """Ensure each test runs with a clean global session handle."""
    session._active_session = None
    yield
    session._active_session = None


def test_auto_start_resolves_filter_chain(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    trace_dir = tmp_path / "trace-output"
    filter_dir = tmp_path / "filters"
    filter_dir.mkdir()
    default_filter = filter_dir / "default.toml"
    default_filter.write_text("# default\n", encoding="utf-8")
    override_filter = filter_dir / "override.toml"
    override_filter.write_text("# override\n", encoding="utf-8")

    state: dict[str, bool] = {"active": False}
    captured_filters: list[list[str] | None] = []

    def fake_start_backend(
        path: str,
        fmt: str,
        activation: str | None,
        filters: list[str] | None,
    ) -> None:
        state["active"] = True
        captured_filters.append(filters)

    def fake_stop_backend() -> None:
        state["active"] = False

    monkeypatch.setenv(auto_start.ENV_TRACE_PATH, str(trace_dir))
    monkeypatch.setenv(
        auto_start.ENV_TRACE_FILTER, f"{default_filter}//{override_filter}"
    )

    monkeypatch.setattr(session, "_start_backend", fake_start_backend)
    monkeypatch.setattr(session, "_stop_backend", fake_stop_backend)
    monkeypatch.setattr(session, "_flush_backend", lambda: None)
    monkeypatch.setattr(session, "_is_tracing_backend", lambda: bool(state["active"]))
    monkeypatch.setattr(session, "_configure_policy_from_env", lambda: None)
    monkeypatch.setattr(backend, "configure_policy_from_env", lambda: None)

    auto_start.auto_start_from_env()

    assert len(captured_filters) == 1
    assert captured_filters[0] == [
        str(default_filter.resolve()),
        str(override_filter.resolve()),
    ]
    assert session._active_session is not None

    session.stop()
    assert state["active"] is False
