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
    start_tracing(str(tmp_path), "json", None)
    with pytest.raises(UsageError) as excinfo:
        start_tracing(str(tmp_path), "json", None)
    err = excinfo.value
    assert getattr(err, "code") == "ERR_ALREADY_TRACING"
    assert "tracing already active" in str(err)


def test_exception_reexport_matches_underlying_type() -> None:
    assert codetracer.UsageError is UsageError
