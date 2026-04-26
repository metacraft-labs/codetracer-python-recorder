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
