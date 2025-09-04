import runpy
import sys
from pathlib import Path

import pytest


def test_fail_fast_when_frame_access_fails(tmp_path: Path):
    # Import the built extension module
    import codetracer_python_recorder as cpr

    # Prepare a simple program that triggers a Python function call
    prog = tmp_path / "prog.py"
    prog.write_text(
        """
def f():
    return 1

f()
"""
    )

    # Monkeypatch sys._getframe to simulate a failure when capturing args
    original_getframe = getattr(sys, "_getframe")

    def boom(*_args, **_kwargs):  # pragma: no cover - intentionally fails
        raise RuntimeError("boom: _getframe disabled")

    sys._getframe = boom  # type: ignore[attr-defined]

    try:
        # Start tracing; activate only for our program path so stray imports don't trigger
        cpr.start_tracing(str(tmp_path), "json", activation_path=str(prog))

        with pytest.raises(RuntimeError) as excinfo:
            runpy.run_path(str(prog), run_name="__main__")

        # Ensure the error surfaced clearly and didn’t get swallowed
        assert "_getframe" in str(excinfo.value) or "boom" in str(excinfo.value)
    finally:
        # Restore state
        sys._getframe = original_getframe  # type: ignore[attr-defined]
        cpr.stop_tracing()

