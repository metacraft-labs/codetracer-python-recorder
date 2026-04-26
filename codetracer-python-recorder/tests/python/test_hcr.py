"""HCR (Hot Code Reload) validation tests.

Records a Python program that reloads a module mid-execution and verifies
that the CTFS trace captures both pre- and post-reload behaviour correctly.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures" / "hcr"

# Resolve ct-print: prefer CT_PRINT env var, fall back to sibling repo.
CT_PRINT = os.environ.get(
    "CT_PRINT",
    str(Path(__file__).resolve().parents[4] / "codetracer-trace-format-nim" / "ct-print"),
)

STEP_PATTERN = re.compile(
    r"^step=(\d+)\s+value=(-?\d+)\s+delta=(-?\d+)\s+total=(-?\d+)$"
)


def _prepare_env() -> dict[str, str]:
    env = os.environ.copy()
    pythonpath = env.get("PYTHONPATH", "")
    root = str(REPO_ROOT)
    env["PYTHONPATH"] = root if not pythonpath else os.pathsep.join([root, pythonpath])
    return env


def _run_recorder(
    args: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "codetracer_python_recorder", *args],
        cwd=cwd,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )


def _run_ct_print(
    ct_file: Path,
    flag: str,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [CT_PRINT, flag, str(ct_file)],
        capture_output=True,
        text=True,
        check=True,
    )


@pytest.fixture()
def hcr_workdir(tmp_path: Path) -> Path:
    """Copy HCR fixture files into a temp directory and prepare mymodule.py."""
    for name in ("hcr_test_program.py", "mymodule_v1.py", "mymodule_v2.py"):
        shutil.copy2(FIXTURES_DIR / name, tmp_path / name)
    # Start with v1 as the active module
    shutil.copy2(FIXTURES_DIR / "mymodule_v1.py", tmp_path / "mymodule.py")
    return tmp_path


def _parse_step_lines(stdout: str) -> list[tuple[int, int, int, int]]:
    """Extract (step, value, delta, total) tuples from recorder stdout."""
    steps = []
    for line in stdout.splitlines():
        m = STEP_PATTERN.match(line.strip())
        if m:
            steps.append(tuple(int(g) for g in m.groups()))
    return steps


def _find_ct_file(trace_dir: Path) -> Path:
    """Find the single .ct file in the trace output directory."""
    ct_files = list(trace_dir.glob("*.ct"))
    assert len(ct_files) >= 1, (
        f"No .ct files found in {trace_dir}, "
        f"contents: {list(trace_dir.iterdir())}"
    )
    return ct_files[0]


def _find_reload_index(stdout: str) -> int | None:
    """Return the 0-based line index of RELOAD_APPLIED in the output lines."""
    for i, line in enumerate(stdout.splitlines()):
        if line.strip() == "RELOAD_APPLIED":
            return i
    return None


class TestHCR:
    """Hot Code Reload tests using CTFS binary traces."""

    def test_hcr_record_and_verify(self, hcr_workdir: Path) -> None:
        """Record the HCR test program, verify stdout formulas and CTFS trace."""
        trace_dir = hcr_workdir / "trace"
        env = _prepare_env()

        args = [
            "--out-dir", str(trace_dir),
            "--format", "ctfs",
            "--on-recorder-error", "disable",
            "--require-trace",
            "--keep-partial-trace",
            str(hcr_workdir / "hcr_test_program.py"),
        ]

        result = _run_recorder(args, cwd=hcr_workdir, env=env)
        assert result.returncode == 0

        stdout = result.stdout

        # --- Verify .ct file was produced ---
        ct_file = _find_ct_file(trace_dir)
        assert ct_file.stat().st_size > 0, "trace .ct file should not be empty"

        # --- Verify 12 step lines in stdout ---
        steps = _parse_step_lines(stdout)
        assert len(steps) == 12, (
            f"Expected 12 step lines, got {len(steps)}. stdout:\n{stdout}"
        )

        # --- Verify RELOAD_APPLIED marker position ---
        reload_idx = _find_reload_index(stdout)
        assert reload_idx is not None, (
            f"RELOAD_APPLIED marker not found in stdout:\n{stdout}"
        )
        # The reload should appear after step 6 output and before step 7 output.
        # Find which step lines are before/after the reload marker.
        output_lines = stdout.splitlines()
        steps_before_reload = []
        steps_after_reload = []
        for i, line in enumerate(output_lines):
            m = STEP_PATTERN.match(line.strip())
            if m:
                step_num = int(m.group(1))
                if i < reload_idx:
                    steps_before_reload.append(step_num)
                else:
                    steps_after_reload.append(step_num)

        assert steps_before_reload == [1, 2, 3, 4, 5, 6], (
            f"Expected steps 1-6 before reload, got {steps_before_reload}"
        )
        assert steps_after_reload == [7, 8, 9, 10, 11, 12], (
            f"Expected steps 7-12 after reload, got {steps_after_reload}"
        )

        # --- Verify v1 formulas for steps 1-6 ---
        history: list[int] = []
        for step, value, delta, total in steps[:6]:
            expected_value = step * 2  # v1 compute
            expected_delta = expected_value + step  # v1 transform
            history.append(expected_delta)
            expected_total = sum(history)  # v1 aggregate

            assert value == expected_value, (
                f"step={step}: v1 compute expected {expected_value}, got {value}"
            )
            assert delta == expected_delta, (
                f"step={step}: v1 transform expected {expected_delta}, got {delta}"
            )
            assert total == expected_total, (
                f"step={step}: v1 aggregate expected {expected_total}, got {total}"
            )

        # --- Verify v2 formulas for steps 7-12 ---
        # history carries over from v1 steps
        for step, value, delta, total in steps[6:]:
            expected_value = step * 3  # v2 compute
            expected_delta = expected_value - step  # v2 transform
            history.append(expected_delta)
            expected_total = max(history)  # v2 aggregate (max over full history)

            assert value == expected_value, (
                f"step={step}: v2 compute expected {expected_value}, got {value}"
            )
            assert delta == expected_delta, (
                f"step={step}: v2 transform expected {expected_delta}, got {delta}"
            )
            assert total == expected_total, (
                f"step={step}: v2 aggregate expected {expected_total}, got {total}"
            )

    def test_hcr_ct_print_summary(self, hcr_workdir: Path) -> None:
        """Verify ct-print --summary reports non-zero steps, calls, values."""
        if not Path(CT_PRINT).exists():
            pytest.skip(f"ct-print not found at {CT_PRINT}")

        trace_dir = hcr_workdir / "trace"
        env = _prepare_env()

        args = [
            "--out-dir", str(trace_dir),
            "--format", "ctfs",
            "--on-recorder-error", "disable",
            "--require-trace",
            "--keep-partial-trace",
            str(hcr_workdir / "hcr_test_program.py"),
        ]

        result = _run_recorder(args, cwd=hcr_workdir, env=env)
        assert result.returncode == 0

        ct_file = _find_ct_file(trace_dir)

        # Run ct-print --summary
        summary_result = _run_ct_print(ct_file, "--summary")
        assert summary_result.returncode == 0
        summary = summary_result.stdout
        assert summary.strip(), "ct-print --summary produced empty output"

        # The summary should mention steps/calls/values with non-zero counts.
        # Parse numbers from the summary output to verify the trace is non-trivial.
        numbers = [int(n) for n in re.findall(r"\d+", summary)]
        assert any(n > 0 for n in numbers), (
            f"ct-print --summary should report non-zero counts. Output:\n{summary}"
        )


def _record_and_parse_json(hcr_workdir: Path) -> dict:
    """Record the HCR test program and return parsed ct-print --json output."""
    trace_dir = hcr_workdir / "trace"
    env = _prepare_env()

    args = [
        "--out-dir", str(trace_dir),
        "--format", "ctfs",
        "--on-recorder-error", "disable",
        "--require-trace",
        "--keep-partial-trace",
        str(hcr_workdir / "hcr_test_program.py"),
    ]

    result = _run_recorder(args, cwd=hcr_workdir, env=env)
    assert result.returncode == 0

    ct_file = _find_ct_file(trace_dir)
    assert ct_file.stat().st_size > 0

    proc = subprocess.run(
        [CT_PRINT, "--json", str(ct_file)],
        capture_output=True,
        timeout=30,
    )
    assert proc.returncode == 0, f"ct-print --json failed: {proc.stderr}"
    # ct-print may emit CBOR bytes in value fields; use surrogateescape
    # to round-trip without crashing, though --json mode should be clean.
    stdout = proc.stdout.decode("utf-8", errors="replace")
    return json.loads(stdout)


@pytest.fixture()
def hcr_trace_json(hcr_workdir: Path) -> dict:
    """Record the HCR program and return the parsed ct-print --json dict.

    Skips the entire test if ct-print is not available.
    """
    if not Path(CT_PRINT).exists():
        pytest.skip(f"ct-print not found at {CT_PRINT}")
    return _record_and_parse_json(hcr_workdir)


class TestHCRTraceContent:
    """Verify the binary CTFS trace content via ct-print --json."""

    # ------------------------------------------------------------------
    # 1. Step count: the trace must contain steps in both files
    # ------------------------------------------------------------------

    def test_step_count_total(self, hcr_trace_json: dict) -> None:
        """The trace must contain a substantial number of steps."""
        steps = hcr_trace_json["steps"]
        # 155 observed in practice; allow some tolerance for recorder changes
        # but require at least 100 to catch catastrophic regressions.
        assert len(steps) >= 100, (
            f"Expected at least 100 total steps, got {len(steps)}"
        )

    def test_steps_reference_test_program(self, hcr_trace_json: dict) -> None:
        """Steps must reference the test program file."""
        steps = hcr_trace_json["steps"]
        program_steps = [
            s for s in steps if s["path"].endswith("hcr_test_program.py")
        ]
        assert len(program_steps) >= 60, (
            f"Expected at least 60 steps in the test program, "
            f"got {len(program_steps)}"
        )

    def test_steps_reference_mymodule(self, hcr_trace_json: dict) -> None:
        """Steps must reference mymodule.py (the reloaded module)."""
        steps = hcr_trace_json["steps"]
        module_steps = [
            s for s in steps if s["path"].endswith("mymodule.py")
        ]
        # 12 calls * 1 body line each = at least 12, observed ~42
        assert len(module_steps) >= 12, (
            f"Expected at least 12 steps in mymodule.py, "
            f"got {len(module_steps)}"
        )

    # ------------------------------------------------------------------
    # 2. Function definitions
    # ------------------------------------------------------------------

    def test_functions_include_reloadable(self, hcr_trace_json: dict) -> None:
        """The trace must list compute, transform, aggregate functions."""
        functions = hcr_trace_json["functions"]
        for expected in ("compute", "transform", "aggregate"):
            assert expected in functions, (
                f"Function '{expected}' not found in trace functions: "
                f"{functions}"
            )

    def test_functions_include_module_entries(self, hcr_trace_json: dict) -> None:
        """The trace must list module-level entries for main and mymodule."""
        functions = hcr_trace_json["functions"]
        assert "<__main__>" in functions, (
            f"<__main__> not in functions: {functions}"
        )
        assert "<mymodule>" in functions, (
            f"<mymodule> not in functions: {functions}"
        )

    # ------------------------------------------------------------------
    # 3. Call events: exact counts for the reloadable functions
    # ------------------------------------------------------------------

    def test_calls_compute_count(self, hcr_trace_json: dict) -> None:
        """compute() must be called exactly 12 times (once per iteration)."""
        calls = hcr_trace_json["calls"]
        compute_calls = [c for c in calls if c["function"] == "compute"]
        assert len(compute_calls) == 12, (
            f"Expected 12 compute calls, got {len(compute_calls)}"
        )

    def test_calls_transform_count(self, hcr_trace_json: dict) -> None:
        """transform() must be called exactly 12 times."""
        calls = hcr_trace_json["calls"]
        transform_calls = [c for c in calls if c["function"] == "transform"]
        assert len(transform_calls) == 12, (
            f"Expected 12 transform calls, got {len(transform_calls)}"
        )

    def test_calls_aggregate_count(self, hcr_trace_json: dict) -> None:
        """aggregate() must be called exactly 12 times."""
        calls = hcr_trace_json["calls"]
        aggregate_calls = [c for c in calls if c["function"] == "aggregate"]
        assert len(aggregate_calls) == 12, (
            f"Expected 12 aggregate calls, got {len(aggregate_calls)}"
        )

    def test_calls_mymodule_loaded_twice(self, hcr_trace_json: dict) -> None:
        """<mymodule> must appear twice (initial import + reload)."""
        calls = hcr_trace_json["calls"]
        mod_calls = [c for c in calls if c["function"] == "<mymodule>"]
        assert len(mod_calls) == 2, (
            f"Expected 2 <mymodule> calls (import + reload), "
            f"got {len(mod_calls)}"
        )

    def test_calls_total(self, hcr_trace_json: dict) -> None:
        """Total call count: 1 main + 2 module + 36 function = 39."""
        calls = hcr_trace_json["calls"]
        assert len(calls) == 39, (
            f"Expected 39 total calls, got {len(calls)}"
        )

    def test_call_ordering(self, hcr_trace_json: dict) -> None:
        """Call entry steps must be monotonically non-decreasing."""
        calls = hcr_trace_json["calls"]
        # Exclude the top-level <__main__> which wraps everything
        inner_calls = [c for c in calls if c["function"] != "<__main__>"]
        entry_steps = [c["entry_step"] for c in inner_calls]
        for i in range(1, len(entry_steps)):
            assert entry_steps[i] >= entry_steps[i - 1], (
                f"Call entry steps not monotonic at index {i}: "
                f"{entry_steps[i - 1]} > {entry_steps[i]}"
            )

    def test_call_entry_exit_consistency(self, hcr_trace_json: dict) -> None:
        """Every call's exit_step must be >= entry_step."""
        for call in hcr_trace_json["calls"]:
            assert call["exit_step"] >= call["entry_step"], (
                f"Call {call['function']} has exit_step < entry_step: "
                f"{call}"
            )

    # ------------------------------------------------------------------
    # 4. Paths
    # ------------------------------------------------------------------

    def test_paths_include_both_files(self, hcr_trace_json: dict) -> None:
        """Paths must include hcr_test_program.py and mymodule.py."""
        paths = hcr_trace_json["paths"]
        assert any(p.endswith("hcr_test_program.py") for p in paths), (
            f"hcr_test_program.py not in paths: {paths}"
        )
        assert any(p.endswith("mymodule.py") for p in paths), (
            f"mymodule.py not in paths: {paths}"
        )

    def test_paths_count(self, hcr_trace_json: dict) -> None:
        """Exactly 2 source files should be traced."""
        paths = hcr_trace_json["paths"]
        assert len(paths) == 2, (
            f"Expected 2 paths, got {len(paths)}: {paths}"
        )

    # ------------------------------------------------------------------
    # 5. Step source locations
    # ------------------------------------------------------------------

    def test_step_lines_are_valid(self, hcr_trace_json: dict) -> None:
        """All steps must have positive line numbers."""
        for step in hcr_trace_json["steps"]:
            assert step["line"] >= 1, (
                f"Step {step['index']} has invalid line {step['line']}"
            )

    def test_step_path_ids_are_valid(self, hcr_trace_json: dict) -> None:
        """All step path_ids must reference a valid path."""
        num_paths = len(hcr_trace_json["paths"])
        for step in hcr_trace_json["steps"]:
            assert 0 <= step["path_id"] < num_paths, (
                f"Step {step['index']} has invalid path_id {step['path_id']}"
            )

    def test_loop_body_lines_traced(self, hcr_trace_json: dict) -> None:
        """The for-loop body (lines 14-27) must appear in program steps."""
        steps = hcr_trace_json["steps"]
        program_steps = [
            s for s in steps if s["path"].endswith("hcr_test_program.py")
        ]
        lines_hit = {s["line"] for s in program_steps}
        # Line 14 = for loop, 15 = counter += 1, 23 = compute call,
        # 24 = transform call, 27 = print
        for expected_line in (14, 15, 23, 24, 27):
            assert expected_line in lines_hit, (
                f"Line {expected_line} of hcr_test_program.py not traced. "
                f"Lines hit: {sorted(lines_hit)}"
            )

    # ------------------------------------------------------------------
    # 6. Value events: variable names that must be captured
    # ------------------------------------------------------------------

    def test_values_count_substantial(self, hcr_trace_json: dict) -> None:
        """The trace must record a substantial number of value events."""
        values = hcr_trace_json["values"]
        # Observed ~2553; require at least 100 to catch major regressions
        assert len(values) >= 100, (
            f"Expected at least 100 value events, got {len(values)}"
        )

    def test_values_capture_key_variables(self, hcr_trace_json: dict) -> None:
        """Value events must capture counter, value, delta, total, n."""
        values = hcr_trace_json["values"]
        captured_varnames = {v["varname"] for v in values}
        for expected in ("counter", "value", "delta", "total", "n"):
            assert expected in captured_varnames, (
                f"Variable '{expected}' not captured in value events. "
                f"Captured varnames: {sorted(captured_varnames)}"
            )

    def test_values_for_n_in_compute(self, hcr_trace_json: dict) -> None:
        """The parameter 'n' should appear in value events at compute call steps."""
        calls = hcr_trace_json["calls"]
        values = hcr_trace_json["values"]

        compute_entry_steps = {
            c["entry_step"] for c in calls if c["function"] == "compute"
        }
        n_values_at_compute = [
            v for v in values
            if v["varname"] == "n" and v["step"] in compute_entry_steps
        ]
        # Each compute call should capture n; allow some flexibility
        assert len(n_values_at_compute) >= 6, (
            f"Expected n captured at compute entry steps at least 6 times, "
            f"got {len(n_values_at_compute)}"
        )

    # ------------------------------------------------------------------
    # 7. Metadata
    # ------------------------------------------------------------------

    def test_metadata_program_path(self, hcr_trace_json: dict) -> None:
        """Metadata must reference the test program."""
        program = hcr_trace_json["metadata"]["program"]
        assert program.endswith("hcr_test_program.py"), (
            f"Unexpected program in metadata: {program}"
        )

    # ------------------------------------------------------------------
    # 8. Structural integrity across sections
    # ------------------------------------------------------------------

    def test_call_function_ids_are_valid(self, hcr_trace_json: dict) -> None:
        """All call function_ids must reference a valid function."""
        num_functions = len(hcr_trace_json["functions"])
        for call in hcr_trace_json["calls"]:
            assert 0 <= call["function_id"] < num_functions, (
                f"Call has invalid function_id {call['function_id']}: {call}"
            )

    def test_value_steps_are_valid(self, hcr_trace_json: dict) -> None:
        """All value step references must be within the step range."""
        num_steps = len(hcr_trace_json["steps"])
        for val in hcr_trace_json["values"]:
            assert 0 <= val["step"] < num_steps, (
                f"Value references invalid step {val['step']}: {val}"
            )
