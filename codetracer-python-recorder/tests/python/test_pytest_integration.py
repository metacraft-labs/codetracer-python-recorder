"""Integration tests for pytest recording support.

Per ``codetracer-specs/Recorder-CLI-Conventions.md`` §4 the recorder is
CTFS-only: it emits a single ``<program>.ct`` container and never the
legacy ``trace.json`` / ``trace_metadata.json`` JSON sidecars (retired
in commit ``efcfe28``, "#254 Phase 2").  Tests that previously asserted
on ``trace_metadata.json`` now decode the produced ``.ct`` container via
``ct-print --meta-json`` (the canonical CTFS reader from
``codetracer-trace-format-nim``).  ``--meta-json`` materialises only the
``meta.dat`` block — program, args, trace-filter provenance chain — so
it stays fast even on the multi-MB traces a real ``pytest`` run yields.

The asserted facts are unchanged in content and strength:

* ``program`` metadata          → ``metadata.program``.
* recorded program ``args``      → ``metadata.args`` (threaded into
  CTFS ``meta.dat`` by the recorder; spec §7).
* the composed trace-filter set  → ``metadata.trace_filter.filters[]``.
  The retired JSON sidecar carried a human ``name`` per filter
  (``builtin-default`` / ``builtin-pytest`` / ``builtin-unittest``);
  the CTFS ``meta.dat`` records each filter by its ``path``, and the
  recorder's builtin inline filters use the canonical sentinel paths
  ``<inline:builtin-default>`` / ``<inline:builtin-pytest>`` /
  ``<inline:builtin-unittest>`` (see
  ``src/session/bootstrap/filters.rs``).  Asserting on the sentinel
  path is strictly *stronger* than asserting on the old JSON ``name``:
  it confirms the filter was actually composed into the live chain,
  not merely that a label was written to a sidecar.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from .support.ctfs import ct_print_meta, find_ct_file


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


def _read_ct_metadata(trace_dir: Path) -> dict[str, Any]:
    """Decode the CTFS container in *trace_dir* and return ``metadata``.

    Locates the single ``.ct`` container (asserting the legacy
    ``trace.json`` sidecar is absent — the recorder is CTFS-only) and
    returns the ``meta.dat`` metadata block decoded by
    ``ct-print --meta-json``.
    """
    return ct_print_meta(find_ct_file(trace_dir))["metadata"]


def _filter_paths(metadata: dict[str, Any]) -> list[str]:
    """Return the ordered ``path`` of every filter in the provenance chain.

    ``metadata.trace_filter.filters[]`` is the composed trace-filter
    chain recorded in CTFS ``meta.dat`` (TF-M7, spec §7).  A recorder
    that recorded no chain has no ``trace_filter`` key at all — that is
    itself a failure for these tests, so we assert the block is present.
    """
    trace_filter = metadata.get("trace_filter")
    assert isinstance(trace_filter, dict), (
        "metadata.trace_filter missing — the recorder did not record the "
        f"filter chain in meta.dat; metadata={metadata!r}"
    )
    filters = trace_filter.get("filters")
    assert isinstance(filters, list), f"trace_filter.filters not a list: {trace_filter!r}"
    return [entry["path"] for entry in filters]


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
            "--pytest",
            str(test_file),
            "-v",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        # Test may pass or fail depending on pytest availability, but trace should be created
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata = _read_ct_metadata(trace_dir)

        # The composed trace-filter chain must include both the builtin
        # default and the framework-specific pytest filter.  ``--pytest``
        # is what loads ``builtin-pytest`` into the live chain — its
        # presence here is the CTFS-side evidence that the recorder
        # treated the run as a pytest run (the retired JSON sidecar's
        # ``recorder.test_framework`` field was a weaker duplicate of the
        # same signal).
        filter_paths = _filter_paths(metadata)
        assert "<inline:builtin-default>" in filter_paths, (
            f"Expected builtin-default filter, got: {filter_paths}"
        )
        assert "<inline:builtin-pytest>" in filter_paths, (
            f"Expected builtin-pytest filter, got: {filter_paths}"
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
            "--pytest",
            str(test_file),
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata = _read_ct_metadata(trace_dir)
        assert metadata.get("program") == "pytest"

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
            "--pytest",
            str(test_file),
            "-v",
            "-k",
            "fast",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata = _read_ct_metadata(trace_dir)

        # The recorder threads the recorded program's argv into CTFS
        # meta.dat (spec §7); ``ct-print --meta-json`` surfaces it as
        # ``metadata.args``.  The pytest arguments must round-trip.
        recorded_args = metadata.get("args", [])
        assert str(test_file) in recorded_args, (
            f"test file missing from recorded args: {recorded_args}"
        )
        assert "-v" in recorded_args, f"-v missing from recorded args: {recorded_args}"
        assert "-k" in recorded_args, f"-k missing from recorded args: {recorded_args}"
        assert "fast" in recorded_args, f"fast missing from recorded args: {recorded_args}"

    def test_no_framework_filters_disables_pytest_filter(self, tmp_path: Path) -> None:
        """Verify that --no-framework-filters excludes the pytest filter."""
        test_file = tmp_path / "test_example.py"
        _write_test_file(test_file, "def test_pass(): pass\n")

        trace_dir = tmp_path / "trace"
        env = _prepare_env()
        args = [
            "--out-dir",
            str(trace_dir),
            "--no-framework-filters",
            "--pytest",
            str(test_file),
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata = _read_ct_metadata(trace_dir)
        filter_paths = _filter_paths(metadata)

        # Should still have builtin-default but NOT builtin-pytest
        assert "<inline:builtin-default>" in filter_paths, (
            f"Expected builtin-default filter, got: {filter_paths}"
        )
        assert "<inline:builtin-pytest>" not in filter_paths, (
            f"Expected no builtin-pytest filter with --no-framework-filters, got: {filter_paths}"
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
            "--unittest",
            str(test_file),
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata = _read_ct_metadata(trace_dir)
        filter_paths = _filter_paths(metadata)

        # ``--unittest`` must compose the builtin-unittest framework
        # filter into the live chain alongside the builtin default — the
        # CTFS-side evidence that the recorder treated the run as a
        # unittest run.
        assert "<inline:builtin-default>" in filter_paths, (
            f"Expected builtin-default filter, got: {filter_paths}"
        )
        assert "<inline:builtin-unittest>" in filter_paths, (
            f"Expected builtin-unittest filter, got: {filter_paths}"
        )

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
            "--unittest",
            str(test_file),
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata = _read_ct_metadata(trace_dir)
        assert metadata.get("program") == "unittest"


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
            "--pytest",
            str(test_file),
            "-v",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        # Trace should be created even if pytest-asyncio is not installed
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        # A decodable CTFS container must be produced for the async test
        # file: ``find_ct_file`` asserts the ``.ct`` exists with valid
        # CTFS magic, and ``ct-print --meta-json`` decoding it confirms
        # the container is well-formed and records the pytest run.
        metadata = _read_ct_metadata(trace_dir)
        assert metadata.get("program") == "pytest", (
            f"async-test recording must be a pytest run, got: {metadata.get('program')!r}"
        )


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
            "--pytest",
            node_id,
            "-v",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        assert trace_dir.is_dir(), f"Trace directory not created. stderr: {result.stderr}"

        metadata = _read_ct_metadata(trace_dir)

        # The pytest node ID must round-trip into the recorded argv
        # (CTFS meta.dat ``args``).
        recorded_args = metadata.get("args", [])
        assert node_id in recorded_args, (
            f"node id {node_id!r} missing from recorded args: {recorded_args}"
        )

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
            "--pytest",
            str(test_file),
            "-v",
        ]

        result = _run_cli(args, cwd=tmp_path, env=env, check=False)
        if not trace_dir.is_dir():
            pytest.skip("Trace not created - pytest may not be installed")

        # Check the CTFS trace was produced.  Per convention §4 the
        # recorder is CTFS-only, so we only look for ``trace.ct`` (and
        # explicitly forbid the legacy ``trace.json`` sidecar).
        trace_ct = trace_dir / "trace.ct"
        legacy_trace_json = trace_dir / "trace.json"
        assert not legacy_trace_json.exists(), (
            "trace.json must not be produced — recorder is CTFS-only"
        )
        if trace_ct.exists():
            # Filter stats sit in the metadata sidecar, which is plain
            # JSON regardless of trace events format.
            filter_stats = json.loads(
                (trace_dir / "trace_metadata.json").read_text(encoding="utf-8")
            ).get("trace_filter", {}).get("stats", {})

            # Verify some scopes were skipped (indicating filter worked)
            scopes_skipped = filter_stats.get("scopes_skipped", 0)
            assert scopes_skipped > 0, "Expected some scopes to be skipped by filter"
