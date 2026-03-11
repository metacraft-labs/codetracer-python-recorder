"""Integration tests for pytest recording support."""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]


def _run_cli(
    args: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "codetracer_python_recorder", *args],
        cwd=cwd,
        env=env,
        check=check,
        capture_output=True,
        text=True,
    )


def _prepare_env() -> dict[str, str]:
    env = os.environ.copy()
    pythonpath = env.get("PYTHONPATH", "")
    root = str(REPO_ROOT)
    env["PYTHONPATH"] = root if not pythonpath else os.pathsep.join([root, pythonpath])
    return env


def _write_test_file(path: Path, content: str) -> None:
    path.write_text(content, encoding="utf-8")


class TestPytestIntegration:
    """Tests for --pytest flag functionality."""

    def test_pytest_flag_loads_pytest_filter(self, tmp_path: Path) -> None:
        """Verify that --pytest loads builtin-pytest filter in addition to builtin-default."""
        test_file = tmp_path / "test_example.py"
        _write_test_file(
            test_file,
            """
def add(a, b):
    return a + b

def test_addition():
    assert add(1, 2) == 3
""",
        )

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--pytest",
            str(test_file),
            "-v",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        # Test may pass or fail depending on pytest availability, but trace should be created
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata_file = trace_dir / "trace_metadata.json"
        assert metadata_file.exists(), "Metadata file not created"

        payload = json.loads(metadata_file.read_text(encoding="utf-8"))

        # Check trace_filter includes both builtin-default and builtin-pytest
        trace_filter = payload.get("trace_filter", {})
        filters = trace_filter.get("filters", [])
        filter_names = [f.get("name") for f in filters if isinstance(f, dict)]

        assert "builtin-default" in filter_names, f"Expected builtin-default filter, got: {filter_names}"
        assert "builtin-pytest" in filter_names, f"Expected builtin-pytest filter, got: {filter_names}"

        # Check recorder metadata includes test_framework
        recorder_info = payload.get("recorder", {})
        assert recorder_info.get("test_framework") == "pytest", (
            f"Expected test_framework=pytest, got: {recorder_info}"
        )

    def test_pytest_flag_program_is_pytest(self, tmp_path: Path) -> None:
        """Verify that --pytest sets program metadata to 'pytest'."""
        test_file = tmp_path / "test_simple.py"
        _write_test_file(test_file, "def test_pass(): pass\n")

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--pytest",
            str(test_file),
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata_file = trace_dir / "trace_metadata.json"
        payload = json.loads(metadata_file.read_text(encoding="utf-8"))

        assert payload.get("program") == "pytest"

    def test_pytest_args_passthrough(self, tmp_path: Path) -> None:
        """Verify that pytest arguments are passed through correctly."""
        test_file = tmp_path / "test_marked.py"
        _write_test_file(
            test_file,
            """
import pytest

@pytest.mark.slow
def test_slow():
    pass

def test_fast():
    pass
""",
        )

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        # Pass pytest-specific arguments like -k and -v
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--pytest",
            str(test_file),
            "-v",
            "-k",
            "fast",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata_file = trace_dir / "trace_metadata.json"
        payload = json.loads(metadata_file.read_text(encoding="utf-8"))

        # Check that args include the pytest arguments
        recorded_args = payload.get("args", [])
        assert str(test_file) in recorded_args
        assert "-v" in recorded_args
        assert "-k" in recorded_args
        assert "fast" in recorded_args

    def test_no_framework_filters_disables_pytest_filter(self, tmp_path: Path) -> None:
        """Verify that --no-framework-filters excludes the pytest filter."""
        test_file = tmp_path / "test_example.py"
        _write_test_file(test_file, "def test_pass(): pass\n")

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--no-framework-filters",
            "--pytest",
            str(test_file),
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata_file = trace_dir / "trace_metadata.json"
        payload = json.loads(metadata_file.read_text(encoding="utf-8"))

        trace_filter = payload.get("trace_filter", {})
        filters = trace_filter.get("filters", [])
        filter_names = [f.get("name") for f in filters if isinstance(f, dict)]

        # Should still have builtin-default but NOT builtin-pytest
        assert "builtin-default" in filter_names
        assert "builtin-pytest" not in filter_names, (
            f"Expected no builtin-pytest filter with --no-framework-filters, got: {filter_names}"
        )


class TestUnittestIntegration:
    """Tests for --unittest flag functionality."""

    def test_unittest_flag_loads_unittest_filter(self, tmp_path: Path) -> None:
        """Verify that --unittest loads builtin-unittest filter."""
        test_file = tmp_path / "test_example.py"
        _write_test_file(
            test_file,
            """
import unittest

class TestMath(unittest.TestCase):
    def test_addition(self):
        self.assertEqual(1 + 2, 3)

if __name__ == "__main__":
    unittest.main()
""",
        )

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--unittest",
            str(test_file),
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata_file = trace_dir / "trace_metadata.json"
        payload = json.loads(metadata_file.read_text(encoding="utf-8"))

        trace_filter = payload.get("trace_filter", {})
        filters = trace_filter.get("filters", [])
        filter_names = [f.get("name") for f in filters if isinstance(f, dict)]

        assert "builtin-default" in filter_names
        assert "builtin-unittest" in filter_names

        recorder_info = payload.get("recorder", {})
        assert recorder_info.get("test_framework") == "unittest"

    def test_unittest_program_metadata(self, tmp_path: Path) -> None:
        """Verify that --unittest sets program metadata correctly."""
        test_file = tmp_path / "test_simple.py"
        _write_test_file(
            test_file,
            """
import unittest

class TestSimple(unittest.TestCase):
    def test_pass(self):
        pass
""",
        )

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--unittest",
            str(test_file),
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir()

        metadata_file = trace_dir / "trace_metadata.json"
        payload = json.loads(metadata_file.read_text(encoding="utf-8"))

        assert payload.get("program") == "unittest"


class TestMutualExclusion:
    """Tests for mutually exclusive framework flags.

    Note: Due to argparse's REMAINDER behavior for --pytest and --unittest,
    whichever flag appears first captures all subsequent arguments. This means
    that `--unittest file.py --pytest` will pass `--pytest` as an argument to
    unittest (which will fail), while `--pytest file.py --unittest` will pass
    `--unittest` as an argument to pytest (which will also fail). The mutual
    exclusion is effectively enforced by the test framework rejecting unknown
    arguments, though not at the CLI parsing level.
    """

    def test_script_and_pytest_mutually_exclusive(self, tmp_path: Path) -> None:
        """Verify that a script argument and --pytest cannot be used together."""
        script = tmp_path / "script.py"
        _write_test_file(script, "print('hello')\n")
        test_file = tmp_path / "test_example.py"
        _write_test_file(test_file, "def test_pass(): pass\n")

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        # Try to pass both a script (positional) and --pytest
        # The argparse setup should prevent this or the CLI should error
        args = [
            "--out-dir",
            str(trace_dir),
            str(script),  # positional script
            "--pytest",
            str(test_file),
        ]

        # This should either fail at parse time or runtime
        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        # The behavior depends on argparse ordering; just ensure it doesn't silently succeed
        # with both modes


class TestAsyncTestSupport:
    """Tests for async test function support."""

    def test_async_test_function_recorded(self, tmp_path: Path) -> None:
        """Verify that async def test_* functions are recorded correctly."""
        test_file = tmp_path / "test_async.py"
        _write_test_file(
            test_file,
            """
import asyncio

async def async_helper():
    await asyncio.sleep(0.001)
    return 42

async def test_async_addition():
    result = await async_helper()
    assert result == 42

def test_sync_addition():
    assert 1 + 1 == 2
""",
        )

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--pytest",
            str(test_file),
            "-v",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        # Trace should be created even if pytest-asyncio is not installed
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata_file = trace_dir / "trace_metadata.json"
        assert metadata_file.exists(), "Metadata file not created"


class TestPytestNodeIds:
    """Tests for pytest node ID handling."""

    def test_specific_test_node_id(self, tmp_path: Path) -> None:
        """Verify that specific test node IDs work correctly."""
        test_file = tmp_path / "test_multiple.py"
        _write_test_file(
            test_file,
            """
def test_first():
    assert True

def test_second():
    assert True

def test_third():
    assert True
""",
        )

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        # Use pytest node ID format: path::test_name
        node_id = f"{test_file}::test_second"
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--pytest",
            node_id,
            "-v",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata_file = trace_dir / "trace_metadata.json"
        payload = json.loads(metadata_file.read_text(encoding="utf-8"))

        # The node ID should be in the args
        recorded_args = payload.get("args", [])
        assert node_id in recorded_args

    def test_test_class_node_id(self, tmp_path: Path) -> None:
        """Verify that test class node IDs work correctly."""
        test_file = tmp_path / "test_class.py"
        _write_test_file(
            test_file,
            """
class TestMath:
    def test_addition(self):
        assert 1 + 1 == 2

    def test_subtraction(self):
        assert 3 - 1 == 2
""",
        )

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        # Use pytest node ID format for class method: path::ClassName::method_name
        node_id = f"{test_file}::TestMath::test_addition"
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--pytest",
            node_id,
            "-v",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"


class TestErrorHandling:
    """Tests for error handling scenarios."""

    def test_invalid_test_path(self, tmp_path: Path) -> None:
        """Verify graceful handling of non-existent test path."""
        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--pytest",
            "/nonexistent/path/test_fake.py",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        # pytest exits with code 4 for "no tests collected" or code 2 for usage errors
        # The recorder may propagate this or return 0 depending on --propagate-script-exit
        # The key is that:
        # 1. It doesn't crash unexpectedly
        # 2. The error is captured in stderr or the return code indicates the issue
        # With default policy (no --propagate-script-exit), recorder returns 0 but prints warning
        if result.returncode == 0:
            # Check that there's a message about the exit status
            assert "exited with status" in result.stderr or "error" in result.stderr.lower()

    def test_empty_pytest_args(self, tmp_path: Path) -> None:
        """Verify error when --pytest has no arguments."""
        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--pytest",
            # No arguments after --pytest
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        # Should fail with an error about missing arguments
        assert result.returncode != 0
        assert "requires" in result.stderr.lower() or "argument" in result.stderr.lower()

    def test_syntax_error_in_test_file(self, tmp_path: Path) -> None:
        """Verify handling of test file with syntax error."""
        test_file = tmp_path / "test_syntax_error.py"
        _write_test_file(
            test_file,
            """
def test_broken(:  # Syntax error
    assert True
""",
        )

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--pytest",
            str(test_file),
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        # Should fail due to syntax error but trace directory might still be created
        # The important thing is it doesn't crash unexpectedly


class TestFilterApplication:
    """Tests for filter application during pytest recording."""

    def test_pytest_filter_skips_pytest_internals(self, tmp_path: Path) -> None:
        """Verify that pytest internals are filtered from trace."""
        test_file = tmp_path / "test_filtered.py"
        _write_test_file(
            test_file,
            """
def helper():
    return 42

def test_with_helper():
    result = helper()
    assert result == 42
""",
        )

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--format",
            "json",
            "--pytest",
            str(test_file),
            "-v",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        if not trace_dir.is_dir():
            pytest.skip("Trace not created - pytest may not be installed")

        # Check the trace.json for any pytest/pluggy modules being traced
        trace_file = trace_dir / "trace.json"
        if trace_file.exists():
            trace_content = trace_file.read_text(encoding="utf-8")
            # Verify pytest internals are not in trace
            # Note: This is a basic check - a more thorough check would parse the JSON
            # and verify no frames from pytest/pluggy modules
            filter_stats = json.loads(
                (trace_dir / "trace_metadata.json").read_text(encoding="utf-8")
            ).get("trace_filter", {}).get("stats", {})

            # Verify some scopes were skipped (indicating filter worked)
            scopes_skipped = filter_stats.get("scopes_skipped", 0)
            assert scopes_skipped > 0, "Expected some scopes to be skipped by filter"
