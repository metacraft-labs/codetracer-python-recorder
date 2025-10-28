"""Unit tests for runtime policy configuration."""
from __future__ import annotations

from pathlib import Path

import pytest

import codetracer_python_recorder as codetracer
from codetracer_python_recorder import session as session_api


@pytest.fixture(autouse=True)
def reset_policy() -> None:
    codetracer.configure_policy(
        on_recorder_error="abort",
        require_trace=False,
        keep_partial_trace=False,
        log_level="",
        log_file="",
        json_errors=False,
        module_name_from_globals=True,
        propagate_script_exit=False,
    )
    yield
    codetracer.configure_policy(
        on_recorder_error="abort",
        require_trace=False,
        keep_partial_trace=False,
        log_level="",
        log_file="",
        json_errors=False,
        module_name_from_globals=True,
        propagate_script_exit=False,
    )


def test_configure_policy_overrides_values(tmp_path: Path) -> None:
    target_log = tmp_path / "recorder.log"
    codetracer.configure_policy(
        on_recorder_error="disable",
        require_trace=True,
        keep_partial_trace=True,
        log_level="info",
        log_file=str(target_log),
        json_errors=True,
        module_name_from_globals=True,
        propagate_script_exit=True,
    )

    snapshot = codetracer.policy_snapshot()
    assert snapshot["on_recorder_error"] == "disable"
    assert snapshot["require_trace"] is True
    assert snapshot["keep_partial_trace"] is True
    assert snapshot["log_level"] == "info"
    assert snapshot["log_file"] == str(target_log)
    assert snapshot["json_errors"] is True
    assert snapshot["module_name_from_globals"] is True
    assert snapshot["propagate_script_exit"] is True


def test_policy_env_configuration(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv("CODETRACER_ON_RECORDER_ERROR", "disable")
    monkeypatch.setenv("CODETRACER_REQUIRE_TRACE", "true")
    monkeypatch.setenv("CODETRACER_KEEP_PARTIAL_TRACE", "1")
    monkeypatch.setenv("CODETRACER_LOG_LEVEL", "debug")
    log_file = tmp_path / "policy.log"
    monkeypatch.setenv("CODETRACER_LOG_FILE", str(log_file))
    monkeypatch.setenv("CODETRACER_JSON_ERRORS", "yes")
    monkeypatch.setenv("CODETRACER_MODULE_NAME_FROM_GLOBALS", "true")
    monkeypatch.setenv("CODETRACER_PROPAGATE_SCRIPT_EXIT", "true")

    codetracer.configure_policy_from_env()

    snapshot = codetracer.policy_snapshot()
    assert snapshot["on_recorder_error"] == "disable"
    assert snapshot["require_trace"] is True
    assert snapshot["keep_partial_trace"] is True
    assert snapshot["log_level"] == "debug"
    assert snapshot["log_file"] == str(log_file)
    assert snapshot["json_errors"] is True
    assert snapshot["module_name_from_globals"] is True
    assert snapshot["propagate_script_exit"] is True


def test_clearing_log_configuration(tmp_path: Path) -> None:
    codetracer.configure_policy(log_level="debug", log_file=str(tmp_path / "log.txt"))
    codetracer.configure_policy(log_level="", log_file="")
    snapshot = codetracer.policy_snapshot()
    assert snapshot["log_level"] is None
    assert snapshot["log_file"] is None


def test_session_start_applies_policy_overrides(tmp_path: Path) -> None:
    policy = {
        "on_recorder_error": "disable",
        "log_file": tmp_path / "policy.log",
        "json_errors": True,
        "module_name_from_globals": True,
        "propagate_script_exit": True,
    }

    session = session_api.start(tmp_path, policy=policy, apply_env_policy=False)
    try:
        snapshot = codetracer.policy_snapshot()
        assert snapshot["on_recorder_error"] == "disable"
        assert snapshot["log_file"] == str(tmp_path / "policy.log")
        assert snapshot["json_errors"] is True
        assert snapshot["module_name_from_globals"] is True
        assert snapshot["propagate_script_exit"] is True
    finally:
        session.stop()


def test_session_start_respects_env_policy(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv("CODETRACER_REQUIRE_TRACE", "true")

    session = session_api.start(tmp_path)
    try:
        snapshot = codetracer.policy_snapshot()
        assert snapshot["require_trace"] is True
    finally:
        session.stop()
