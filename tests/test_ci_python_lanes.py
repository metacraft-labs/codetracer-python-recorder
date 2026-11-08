"""A CI job named for a Python version tests on that version.

``ci.yml``'s test matrix runs one lane per supported interpreter. Its "Python
3.13" lane used to prepare with ``just venv 3.13 dev``: ``venv`` passed ``-p
3.13`` to one ``uv sync``, and the very next command, ``dev``, ran ``uv run``,
which re-resolved the interpreter from ``.python-version`` (3.12) and rebuilt
``.venv`` on it. Every later step ran on 3.12, so the matrix tested 3.12 twice
and 3.13 never, while the job was named "Testing on Python 3.13".

The repair is that a job naming a version sets ``UV_PYTHON`` for the whole job
(every uv invocation reads it) and runs ``just check-python <version>`` before
its first test step. This file checks both, in two ways:

* ``test_workflows_*`` read every workflow and enforce the shape; synthetic
  workflows below are positive controls, so a checker that saw nothing fails.
* ``test_lane_*`` replay each such job's own preparation and check steps,
  with the job's own ``env``, through the real ``just``, the real Justfile and
  the real ``uv``, and then ask ``uv run`` -- what every test recipe calls --
  which interpreter it resolves.

The replay runs against a stub uv project in a temporary directory, not this
repository. That is the one substitution here, and it is deliberate: the
interpreter choice is made by ``uv`` from the environment, ``.python-version``
and ``requires-python``, all of which the stub carries verbatim from this
repo; what the stub leaves out is the PyO3 extension, whose build takes
minutes per interpreter and would rewrite this checkout's ``.venv`` under the
test session that is running from it. For the same reason the ``dev`` recipe
(``uv run ... maturin develop``) is replaced in the replay by the ``uv run``
probe that follows, which goes through exactly the interpreter resolution
``dev``'s ``uv run`` does.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

import pytest
import yaml

REPO = Path(__file__).resolve().parents[1]
WORKFLOWS = REPO / ".github" / "workflows"
JUSTFILE = REPO / "Justfile"

# `${{ matrix.python-version }}` in any spacing, and the token it is normalised
# to so that the claim in a job's name and the value in its `env` and
# `check-python` step compare equal.
MATRIX_PYTHON = re.compile(r"\$\{\{\s*matrix\.python-version\s*\}\}")
MATRIX_TOKEN = "MATRIX_PYTHON_VERSION"

# How a job or step name states the interpreter it runs on.
CLAIM = re.compile(r"\bPython\s+(\d+\.\d+|" + MATRIX_TOKEN + r")\b")

# One `just` invocation and its arguments (recipes and recipe parameters).
JUST_CALL = re.compile(r"\bjust((?:[ \t]+[\w.\-]+)+)")

# Recipes that run uv and therefore pick an interpreter.
PREPARE_RECIPES = {"venv", "dev"}
TEST_RECIPES = {
    "test",
    "py-test",
    "cargo-test",
    "coverage",
    "coverage-python",
    "coverage-rust",
    "bench",
    "test-all",
}
CHECK_RECIPE = "check-python"

# Environment that would make the replayed uv use something other than what
# the job under test says: the active test session's venv, and a UV_PYTHON
# set by the CI lane this test itself runs in.
SCRUBBED_ENV = (
    "VIRTUAL_ENV",
    "UV_PYTHON",
    "UV_PROJECT_ENVIRONMENT",
    "UV_PROJECT",
    "UV_NO_SYNC",
    "UV_FROZEN",
    "UV_LOCKED",
    "UV_OFFLINE",
    "CONDA_PREFIX",
    "PYTHONHOME",
    "PYTHONPATH",
)


def normalise(text: str) -> str:
    return MATRIX_PYTHON.sub(MATRIX_TOKEN, text)


def _without_comments(script: str) -> str:
    return "\n".join(line for line in script.splitlines() if not line.lstrip().startswith("#"))


def just_calls(step: dict) -> list[list[str]]:
    """The argument lists of every ``just`` invocation in ``step``'s script."""
    script = normalise(_without_comments(str(step.get("run") or "")))
    return [match.group(1).split() for match in JUST_CALL.finditer(script)]


def runs_any(step: dict, recipes: set[str]) -> bool:
    return any(recipes.intersection(args) for args in just_calls(step))


def check_python_arg(step: dict) -> str | None:
    for args in just_calls(step):
        if CHECK_RECIPE in args:
            index = args.index(CHECK_RECIPE)
            return args[index + 1] if index + 1 < len(args) else ""
    return None


@dataclass
class Lane:
    """A job that names the interpreter it runs on, and runs uv."""

    workflow: str
    job_id: str
    job: dict
    claimed: set[str]

    @property
    def steps(self) -> list[dict]:
        return list(self.job.get("steps") or [])

    def label(self) -> str:
        return f"{self.workflow}: job {self.job_id!r}"


def lanes(workflow_name: str, workflow: dict) -> list[Lane]:
    found: list[Lane] = []
    for job_id, job in (workflow.get("jobs") or {}).items():
        steps = job.get("steps") or []
        if not any(runs_any(step, PREPARE_RECIPES | TEST_RECIPES) for step in steps):
            continue
        names = [str(job.get("name") or "")] + [str(step.get("name") or "") for step in steps]
        claimed = {m.group(1) for name in names for m in CLAIM.finditer(normalise(name))}
        if claimed:
            found.append(Lane(workflow_name, job_id, job, claimed))
    return found


def audit(workflow_name: str, workflow: dict) -> list[str]:
    """Return one message per way a version-named job could run another version."""
    problems: list[str] = []
    for lane in lanes(workflow_name, workflow):
        where = lane.label()
        if len(lane.claimed) != 1:
            problems.append(f"{where} names more than one Python version: {sorted(lane.claimed)}")
            continue
        (version,) = lane.claimed
        env_python = normalise(str((lane.job.get("env") or {}).get("UV_PYTHON", "")))
        if env_python != version:
            problems.append(
                f"{where} is named for Python {version} but its job env sets "
                f"UV_PYTHON={env_python or '<unset>'}; without it, each `uv run` "
                "re-resolves .python-version"
            )
        for index, step in enumerate(lane.steps):
            for args in just_calls(step):
                if "venv" in args and args.index("venv") + 1 < len(args):
                    following = args[args.index("venv") + 1]
                    if re.fullmatch(r"\d+\.\d+|" + MATRIX_TOKEN, following):
                        problems.append(
                            f"{where} step {index} passes {following} to `just venv`, "
                            "which governs one `uv sync` only"
                        )
        first_test = next(
            (i for i, step in enumerate(lane.steps) if runs_any(step, TEST_RECIPES)), None
        )
        first_prepare = next(
            (i for i, step in enumerate(lane.steps) if runs_any(step, PREPARE_RECIPES)), None
        )
        checks = [
            (i, check_python_arg(step))
            for i, step in enumerate(lane.steps)
            if check_python_arg(step) is not None
        ]
        good_checks = [
            i
            for i, arg in checks
            if arg == version
            and (first_prepare is None or i > first_prepare)
            and (first_test is None or i < first_test)
        ]
        if not good_checks:
            problems.append(
                f"{where} is named for Python {version} but no step runs "
                f"`just {CHECK_RECIPE} {version}` after preparing the environment "
                "and before the first test step"
            )
        for i, arg in checks:
            if arg != version:
                problems.append(f"{where} step {i} checks Python {arg!r}, not {version}")
    return problems


def workflow_files() -> list[Path]:
    return sorted(WORKFLOWS.glob("*.yml")) + sorted(WORKFLOWS.glob("*.yaml"))


def load(path: Path) -> dict:
    return yaml.safe_load(path.read_text(encoding="utf-8")) or {}


def all_lanes() -> list[Lane]:
    return [lane for path in workflow_files() for lane in lanes(path.name, load(path))]


# --- the workflows ---------------------------------------------------------


def test_workflows_have_version_named_lanes() -> None:
    """Non-vacuity: ci.yml's matrix lanes and the coverage job are found."""
    found = {(lane.workflow, lane.job_id) for lane in all_lanes()}
    assert ("ci.yml", "nix-tests") in found
    assert ("ci.yml", "coverage") in found


@pytest.mark.parametrize("path", workflow_files(), ids=lambda p: p.name)
def test_workflows_test_on_the_version_they_name(path: Path) -> None:
    assert audit(path.name, load(path)) == []


# --- positive controls for the checker -------------------------------------

UNFIXED_LANE = """
jobs:
  nix-tests:
    name: Testing on Python ${{matrix.python-version}}
    strategy:
      matrix:
        python-version: ["3.12", "3.13"]
    steps:
      - run: dev-exec just venv ${{matrix.python-version}} dev
      - run: dev-exec just cargo-test
      - run: dev-exec just py-test
"""

FIXED_LANE = """
jobs:
  nix-tests:
    name: Testing on Python ${{matrix.python-version}}
    strategy:
      matrix:
        python-version: ["3.12", "3.13"]
    env:
      UV_PYTHON: ${{ matrix.python-version }}
    steps:
      - run: dev-exec just venv dev
      - run: dev-exec just check-python ${{matrix.python-version}}
      - run: dev-exec just cargo-test
      - run: dev-exec just py-test
"""


def test_checker_flags_the_lane_that_tested_3_12_as_3_13() -> None:
    problems = "\n".join(audit("synthetic.yml", yaml.safe_load(UNFIXED_LANE)))
    assert "UV_PYTHON=<unset>" in problems
    assert "to `just venv`" in problems
    assert f"`just {CHECK_RECIPE} {MATRIX_TOKEN}`" in problems


def test_checker_accepts_the_repaired_lane() -> None:
    assert audit("synthetic.yml", yaml.safe_load(FIXED_LANE)) == []


def test_checker_flags_a_check_after_the_tests() -> None:
    workflow = yaml.safe_load(FIXED_LANE)
    steps = workflow["jobs"]["nix-tests"]["steps"]
    steps.append(steps.pop(1))
    assert any("before the first test step" in p for p in audit("synthetic.yml", workflow))


def test_checker_flags_a_fixed_version_job_without_uv_python() -> None:
    workflow = yaml.safe_load(
        """
jobs:
  verify:
    name: Verify
    steps:
      - name: Prepare dev environment (Python 3.13)
        run: nix develop --command bash -c 'just venv dev'
      - run: nix develop --command bash -c 'just check-python 3.13'
      - run: nix develop --command bash -c 'just test'
"""
    )
    assert any("UV_PYTHON=<unset>" in p for p in audit("synthetic.yml", workflow))


# --- replaying the lanes ---------------------------------------------------


@dataclass
class Replay:
    lane: Lane
    version: str

    def expand(self, text: str) -> str:
        return normalise(text).replace(MATRIX_TOKEN, self.version)

    def env(self) -> dict[str, str]:
        env = {k: v for k, v in os.environ.items() if k not in SCRUBBED_ENV}
        for key, value in (self.lane.job.get("env") or {}).items():
            env[str(key)] = self.expand(str(value))
        return env

    def just_calls(self) -> list[list[str]]:
        """The lane's preparation and check invocations, in step order.

        ``dev`` is dropped (see the module docstring); the probe that follows
        the replay makes the same ``uv run`` interpreter resolution.
        """
        calls: list[list[str]] = []
        for step in self.lane.steps:
            if runs_any(step, TEST_RECIPES):
                break
            for args in just_calls(step):
                args = [self.expand(a) for a in args if a != "dev"]
                if set(args) & ({"venv", CHECK_RECIPE}):
                    calls.append(args)
        return calls

    def label(self) -> str:
        return f"{self.lane.label()} as Python {self.version}"


def replays() -> list[Replay]:
    result: list[Replay] = []
    for lane in all_lanes():
        (claimed,) = lane.claimed if len(lane.claimed) == 1 else (None,)
        if claimed == MATRIX_TOKEN:
            matrix = (lane.job.get("strategy") or {}).get("matrix") or {}
            result += [Replay(lane, str(v)) for v in matrix.get("python-version") or []]
        elif claimed:
            result.append(Replay(lane, claimed))
    return result


def make_stub_project(root: Path) -> None:
    """A uv project that resolves its interpreter the way this repo does.

    ``.python-version`` and ``requires-python`` are copied from the repo; the
    members and dependency groups, which only add packages, are left out.
    """
    requires = tomllib.loads((REPO / "pyproject.toml").read_text(encoding="utf-8"))["project"][
        "requires-python"
    ]
    root.mkdir(parents=True)
    (root / "pyproject.toml").write_text(
        "[project]\n"
        'name = "python-lane-stub"\n'
        'version = "0"\n'
        f"requires-python = {requires!r}\n"
        "dependencies = []\n"
        "[tool.uv]\n"
        "package = false\n",
        encoding="utf-8",
    )
    shutil.copy(REPO / ".python-version", root / ".python-version")
    # `dev` runs `uv run --directory codetracer-python-recorder`; the probe
    # below resolves from the same directory.
    (root / "codetracer-python-recorder").mkdir()
    # The real check script, at the path the real recipe names.
    (root / "scripts").mkdir()
    shutil.copy(REPO / "scripts" / "check_python_version.py", root / "scripts")


def run(cmd: list[str], cwd: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True, timeout=600)


def just(args: list[str], cwd: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    tool = shutil.which("just", path=env.get("PATH"))
    assert tool, "`just` is not on PATH; run the tests inside the dev shell"
    return run(
        [tool, "--justfile", str(JUSTFILE), "--working-directory", str(cwd), *args], cwd, env
    )


def resolved_version(cwd: Path, env: dict[str, str]) -> str:
    uv = shutil.which("uv", path=env.get("PATH"))
    assert uv, "`uv` is not on PATH; run the tests inside the dev shell"
    probe = run(
        [
            uv,
            "run",
            "--directory",
            "codetracer-python-recorder",
            "python",
            "-c",
            "import sys; print('%d.%d' % sys.version_info[:2])",
        ],
        cwd,
        env,
    )
    assert probe.returncode == 0, probe.stderr
    return probe.stdout.strip().splitlines()[-1]


def test_lanes_are_replayed() -> None:
    """Non-vacuity: both matrix lanes are replayed, 3.13 among them."""
    replayed = {(r.lane.job_id, r.version) for r in replays()}
    assert ("nix-tests", "3.12") in replayed
    assert ("nix-tests", "3.13") in replayed


@pytest.mark.parametrize("replay", replays(), ids=lambda r: f"{r.lane.job_id}-{r.version}")
def test_lane_runs_uv_on_the_version_it_names(replay: Replay, tmp_path: Path) -> None:
    project = tmp_path / "project"
    make_stub_project(project)
    env = replay.env()
    calls = replay.just_calls()
    assert any("venv" in args for args in calls), f"{replay.label()} never prepares a venv"
    for args in calls:
        result = just(args, project, env)
        assert result.returncode == 0, (
            f"{replay.label()}: `just {' '.join(args)}` failed\n{result.stdout}{result.stderr}"
        )
    assert resolved_version(project, env) == replay.version, (
        f"{replay.label()}: after the lane's own preparation, `uv run` resolves "
        f"Python {resolved_version(project, env)}"
    )


def test_check_python_fails_on_another_interpreter(tmp_path: Path) -> None:
    """The assertion bites: a mismatched interpreter fails the recipe."""
    project = tmp_path / "project"
    make_stub_project(project)
    env = {k: v for k, v in os.environ.items() if k not in SCRUBBED_ENV}
    declared = (REPO / ".python-version").read_text(encoding="utf-8").split()[0]
    assert just(["venv"], project, env).returncode == 0
    assert resolved_version(project, env) == declared
    major, minor = (int(part) for part in declared.split(".")[:2])
    other = f"{major}.{minor + 1}"
    result = just([CHECK_RECIPE, other], project, env)
    assert result.returncode != 0
    assert f"expected Python {other}, but `uv run` resolves Python {declared}" in result.stderr


@pytest.mark.parametrize("bad", ["3", "3.13.1", "", "py3.13"])
def test_check_python_rejects_a_malformed_version(bad: str) -> None:
    sys.path.insert(0, str(REPO / "scripts"))
    try:
        from check_python_version import check
    finally:
        sys.path.pop(0)
    assert check(bad, (3, 13, 0)) is not None
    assert check("3.13", (3, 13, 0)) is None
