"""Tests for the Python exception hierarchy exposed by the Rust module."""
from __future__ import annotations

import pytest

import codetracer_python_recorder as codetracer
from codetracer_python_recorder.codetracer_python_recorder import (
    UsageError,
    is_tracing,
    start_tracing,
    stop_tracing,
)


@pytest.fixture(autouse=True)
def stop_after() -> None:
    yield
    if is_tracing():
        stop_tracing()


def test_start_tracing_raises_usage_error(tmp_path) -> None:
    start_tracing(str(tmp_path), "ctfs", None, None)
    with pytest.raises(UsageError) as excinfo:
        start_tracing(str(tmp_path), "ctfs", None, None)
    err = excinfo.value
    assert getattr(err, "code") == "ERR_ALREADY_TRACING"
    assert "tracing already active" in str(err)


def test_start_tracing_refuses_json_format(tmp_path) -> None:
    """The backend has no JSON trace writer: only the test oracle writes JSON."""
    with pytest.raises(UsageError) as excinfo:
        start_tracing(str(tmp_path), "json", None, None)
    assert getattr(excinfo.value, "code") == "ERR_UNSUPPORTED_FORMAT"
    assert not is_tracing()
    assert not (tmp_path / "trace.json").exists()


def test_exception_reexport_matches_underlying_type() -> None:
    assert codetracer.UsageError is UsageError
