"""CI takes every cross-repo sibling at the revision ``repro.lock`` pins.

The recorder crate reaches ``../codetracer-trace-format`` by Cargo path, and
``codetracer_trace_writer_nim``'s build script compiles
``../codetracer-trace-format-nim``. The revision of each sibling this commit
builds against is recorded in ONE place: the ``deps`` of the committed
``repro.lock``. The only approved way for a workflow to reproduce it is
metacraft-github-actions ``clone-siblings`` (directly, or through
``setup-dev-env``, which calls it), which reads that lock and clones each
sibling to ``$GITHUB_WORKSPACE/../<name>`` at the pinned commit.

Hand-written clone steps kept coming back, and each one picked its own
revision: ``TF_REF="main"`` (a branch codetracer-trace-format does not have,
so the job died before building anything), a ``dev`` fallback that lacked the
locked commit, ``.github/sibling-pins`` overrides, and ``|| true`` around the
checkout so a missing ref silently left the default branch in place. None of
them read the lock. This test fails on any of those shapes, and on a job that
builds the crate without an approved provisioning step before it.

It parses the workflow files and the lock directly; nothing here is mocked.
The checker is exercised against synthetic workflows below as a positive
control, so a parser change that made it see nothing would fail too.
"""

from __future__ import annotations

import re
import tomllib
from collections.abc import Iterable
from pathlib import Path

import pytest
import yaml

REPO = Path(__file__).resolve().parents[1]
WORKFLOWS = REPO / ".github" / "workflows"
LOCK = REPO / "repro.lock"
SIBLING_REPOS = REPO / ".github" / "sibling-repos"

# The two actions that resolve a sibling's revision from the lock. Pinned to
# their owner/repo so a look-alike in another org does not count.
APPROVED_USES = re.compile(
    r"^metacraft-labs/metacraft-github-actions/(?:setup-dev-env|clone-siblings)@"
)

# Commands that compile the recorder crate (or enter a shell that does), and
# therefore need the siblings on disk at the locked revision first.
BUILD_COMMAND = re.compile(
    r"\bcargo\s+(?:build|test|run|nextest|clippy|check|bench|metadata)\b"
    r"|\bmaturin\s+(?:build|develop|publish|sdist)\b"
    r"|\bjust\s+(?:build|test|venv|dev|cargo-test|py-test|lint|coverage|bench)\b"
    r"|\brepro\s+(?:build|test)\b"
    r"|\bnix\s+build\b"
    r"|\bnix\s+develop\b.*?--command\s+(?:bash\s+-c\s+['\"])?just\b"
)
BUILD_ACTIONS = re.compile(r"^PyO3/maturin-action@")

# A repository named in a clone URL (`https://github.com/<owner>/<name>(.git)`,
# `git@github.com:<owner>/<name>`) or a `gh repo clone <owner>/<name>`.
CLONE_URL = re.compile(
    r"github\.com[/:][\w.-]+/([\w.-]+?)(?:\.git)?(?=[\"'\s/]|$)"
    r"|\bgh\s+repo\s+clone\s+[\w.-]+/([\w.-]+)",
    re.MULTILINE,
)
CLONE_COMMAND = re.compile(r"\bgit\s+clone\b|\bgh\s+repo\s+clone\b")

# A GitHub Actions expression inside a `siblings:` list, and the single token
# it is collapsed to before the list is split on whitespace.
EXPRESSION = re.compile(r"\$\{\{.*?\}\}", re.DOTALL)
EXPRESSION_TOKEN = "${{expression}}"


def lock_pinned_siblings(lock_text: str) -> set[str]:
    """Names of the dependencies ``repro.lock`` pins to a revision.

    The repo's own entry (``self = "true"``) is not a sibling.
    """
    deps = tomllib.loads(lock_text).get("lock", {}).get("deps", [])
    return {dep["name"] for dep in deps if dep.get("revision") and dep.get("self") != "true"}


def declared_siblings(text: str) -> list[str]:
    """Entries of ``.github/sibling-repos``: one name per line, ``#`` comments."""
    names: list[str] = []
    for line in text.splitlines():
        names.extend(line.split("#", 1)[0].split())
    return names


def _without_comments(script: str) -> str:
    """Drop whole-line shell comments, which name commands without running them."""
    return "\n".join(line for line in script.splitlines() if not line.lstrip().startswith("#"))


def build_label(step: dict) -> str | None:
    """The command (or action) by which ``step`` builds the crate, if it does."""
    uses = str(step.get("uses") or "")
    if BUILD_ACTIONS.match(uses):
        return uses
    match = BUILD_COMMAND.search(_without_comments(str(step.get("run") or "")))
    return match.group(0) if match else None


def build_steps(workflow: dict) -> int:
    """How many steps of ``workflow`` build the crate (a non-vacuity count)."""
    return sum(
        1
        for job in (workflow.get("jobs") or {}).values()
        for step in job.get("steps") or []
        if build_label(step)
    )


def _sibling_entries(raw: str) -> Iterable[tuple[str, str | None]]:
    """Parse a ``siblings:`` input into ``(name, ref)`` pairs.

    The grammar is clone-siblings': ``name``, ``owner/name``, ``name=ref`` and
    ``name!=ref``; an empty ref after ``=`` means "use the lock". A
    ``${{ ... }}`` expression is collapsed to one token first: the runner
    substitutes it before clone-siblings splits the list, so the spaces
    inside it never separate entries.
    """
    for entry in EXPRESSION.sub(EXPRESSION_TOKEN, raw).split():
        if entry.startswith("#"):
            continue
        spec, sep, ref = entry.partition("=")
        name = spec.rstrip("!").rsplit("/", 1)[-1]
        yield name, (ref if sep and ref else None)


def audit_workflow(workflow: dict, pinned: set[str], required: set[str]) -> list[str]:
    """Return one message per place ``workflow`` departs from the lock.

    ``pinned`` is every sibling the lock names; ``required`` is the subset a
    crate build needs on disk (``.github/sibling-repos``).
    """
    problems: list[str] = []
    for job_id, job in (workflow.get("jobs") or {}).items():
        steps = job.get("steps")
        if not steps:
            # A job-level `uses:` calls a reusable workflow, which provisions
            # (or not) in its own repository.
            continue
        provisioned: set[str] = set()
        for index, step in enumerate(steps):
            where = (
                f"job '{job_id}' step {index + 1} ({step.get('name') or step.get('uses') or 'run'})"
            )
            uses = str(step.get("uses") or "")
            with_ = step.get("with") or {}
            script = _without_comments(str(step.get("run") or ""))

            if "sibling-pins" in yaml.safe_dump(step):
                problems.append(
                    f"{where}: reads .github/sibling-pins; sibling revisions come from repro.lock"
                )

            if APPROVED_USES.match(uses):
                raw = str(with_.get("siblings") or "")
                entries = (
                    list(_sibling_entries(raw))
                    if raw.strip()
                    else [(name, None) for name in required]
                )
                for name, ref in entries:
                    # `${{ ... }}` is a dispatch override that is empty (=> the
                    # lock) unless a human supplies a ref; a literal is a
                    # standing replacement of the pin.
                    if ref is not None and not ref.startswith("${{") and name in pinned:
                        problems.append(
                            f"{where}: pins sibling '{name}' to '{ref}' instead of the revision repro.lock pins"
                        )
                    provisioned.add(name)
                continue

            if uses.startswith("actions/checkout@") and with_.get("repository"):
                name = str(with_["repository"]).rsplit("/", 1)[-1]
                if name in pinned:
                    problems.append(
                        f"{where}: checks out lock-pinned sibling '{name}' with actions/checkout"
                    )
            if "/clone-repo@" in uses:
                name = str(with_.get("repo") or "").rsplit("/", 1)[-1]
                if name in pinned:
                    problems.append(
                        f"{where}: clones lock-pinned sibling '{name}' with clone-repo at '{with_.get('ref')}'"
                    )

            if CLONE_COMMAND.search(script):
                cloned = {m.group(1) or m.group(2) for m in CLONE_URL.finditer(script)}
                for name in sorted(cloned & pinned):
                    problems.append(f"{where}: hand-clones lock-pinned sibling '{name}'")
            for name in pinned:
                # Moving an already-cloned sibling off its pin is the same loss.
                if re.search(
                    rf"\bgit\s+-C\s+[\"']?[^\"'\s]*\.\./{re.escape(name)}[\"']?\s+(?:checkout|fetch|reset|switch|pull)\b",
                    script,
                ):
                    problems.append(
                        f"{where}: moves lock-pinned sibling '{name}' to another revision"
                    )

            build = build_label(step)
            missing = required - provisioned
            if build and missing:
                problems.append(
                    f"{where}: builds the crate ('{build}') before "
                    f"{', '.join(sorted(missing))} is provisioned by setup-dev-env/clone-siblings"
                )
    return problems


def _workflow_files() -> list[Path]:
    return sorted([*WORKFLOWS.glob("*.yml"), *WORKFLOWS.glob("*.yaml")])


def _repo_inputs() -> tuple[set[str], set[str]]:
    pinned = lock_pinned_siblings(LOCK.read_text(encoding="utf-8"))
    required = set(declared_siblings(SIBLING_REPOS.read_text(encoding="utf-8")))
    return pinned, required


def test_declared_siblings_are_pinned_by_the_lock() -> None:
    # clone-siblings refuses a sibling the lock does not name, so a declared
    # sibling missing here would fail every job that provisions it.
    pinned, required = _repo_inputs()
    assert required, ".github/sibling-repos declares no siblings"
    assert required <= pinned, (
        f"repro.lock does not pin {sorted(required - pinned)}; run `repro lock refresh`"
    )


def test_workflows_take_siblings_from_the_lock() -> None:
    pinned, required = _repo_inputs()
    problems: list[str] = []
    builds_seen = 0
    for path in _workflow_files():
        workflow = yaml.safe_load(path.read_text(encoding="utf-8"))
        problems += [f"{path.name}: {p}" for p in audit_workflow(workflow, pinned, required)]
        # Non-vacuity: the audit must actually see the steps that build.
        builds_seen += build_steps(workflow)
    assert builds_seen > 0, "no workflow step was recognised as building the crate"
    assert not problems, "\n".join(problems)


# --- positive controls: the checker itself ---------------------------------

PINNED = {"codetracer-trace-format", "codetracer-trace-format-nim", "codetracer-launcher"}
REQUIRED = {"codetracer-trace-format", "codetracer-trace-format-nim"}


def _audit(text: str) -> list[str]:
    return audit_workflow(yaml.safe_load(text), PINNED, REQUIRED)


def test_checker_flags_the_hand_rolled_clone_this_repo_had() -> None:
    problems = _audit(
        """
jobs:
  verify:
    steps:
      - uses: actions/checkout@v4
      - name: Clone codetracer-trace-format
        run: |
          PINS=".github/sibling-pins"
          TF_REF="main"
          git clone --depth 1 --branch "${TF_REF}" \\
            "https://github.com/metacraft-labs/codetracer-trace-format.git" \\
            "${GITHUB_WORKSPACE}/../codetracer-trace-format"
          git -C "${GITHUB_WORKSPACE}/../codetracer-trace-format" checkout "${TF_REF}" || true
      - name: Clone nim
        run: git clone https://github.com/metacraft-labs/codetracer-trace-format-nim ../codetracer-trace-format-nim
      - uses: metacraft-labs/metacraft-github-actions/setup-dev-env@dev
        with:
          env-flavor: nix
      - run: dev-exec just test
"""
    )
    text = "\n".join(problems)
    assert "sibling-pins" in text
    assert "hand-clones lock-pinned sibling 'codetracer-trace-format'" in text
    assert "hand-clones lock-pinned sibling 'codetracer-trace-format-nim'" in text
    assert "moves lock-pinned sibling 'codetracer-trace-format'" in text
    # The build comes after setup-dev-env, so it is not reported.
    assert "builds the crate" not in text


@pytest.mark.parametrize(
    "command",
    [
        "nix develop --command bash -c 'just test'",
        "dev-exec just venv 3.12 dev",
        "cargo build --release",
        "uv run cargo nextest run --workspace",
        "maturin build --release --out dist",
        "nix build .#default",
        "dev-exec repro build",
    ],
)
def test_checker_flags_a_build_without_provisioning(command: str) -> None:
    problems = _audit(
        f"""
jobs:
  build:
    steps:
      - uses: actions/checkout@v4
      - run: {command}
      - uses: metacraft-labs/metacraft-github-actions/clone-siblings@dev
"""
    )
    assert len(problems) == 1 and "builds the crate" in problems[0], problems


def test_checker_flags_a_partial_or_overridden_sibling_list() -> None:
    problems = _audit(
        """
jobs:
  build:
    steps:
      - uses: metacraft-labs/metacraft-github-actions/clone-siblings@dev
        with:
          siblings: |
            codetracer-trace-format=dev
      - run: cargo build
      - uses: metacraft-labs/metacraft-github-actions/clone-repo@dev
        with:
          repo: metacraft-labs/codetracer-launcher
          ref: dev
"""
    )
    text = "\n".join(problems)
    assert "pins sibling 'codetracer-trace-format' to 'dev'" in text
    assert "before codetracer-trace-format-nim is provisioned" in text
    assert "clones lock-pinned sibling 'codetracer-launcher' with clone-repo" in text


def test_checker_accepts_the_approved_shapes() -> None:
    assert (
        _audit(
            """
jobs:
  default-list:
    steps:
      - uses: actions/checkout@v4
      - uses: metacraft-labs/metacraft-github-actions/setup-dev-env@dev
        with:
          env-flavor: windows-diy
      - run: dev-exec just test
  explicit-list:
    steps:
      - uses: metacraft-labs/metacraft-github-actions/clone-siblings@dev
        with:
          siblings: |
            codetracer-trace-format
            codetracer-trace-format-nim
            codetracer-launcher=${{ inputs.launcher_ref }}
      - uses: PyO3/maturin-action@v1
      - run: nix develop --command bash -c 'just test'
  reusable:
    uses: metacraft-labs/codetracer/.github/workflows/x.yml@dev
  installs-tooling-only:
    steps:
      # Installing maturin, or naming cargo in a comment, builds nothing.
      - run: |
          # cargo build happens in another job
          python3 -m pip install --upgrade pip "maturin>=1.5,<2"
"""
        )
        == []
    )


def test_sibling_list_parsing_keeps_an_expression_in_one_entry() -> None:
    # The spaces inside `${{ ... }}` must not split it into extra "siblings".
    raw = "codetracer-launcher!=${{ inputs.launcher_ref || 'x' }}\nowner/codetracer-trace-format\n"
    assert list(_sibling_entries(raw)) == [
        ("codetracer-launcher", EXPRESSION_TOKEN),
        ("codetracer-trace-format", None),
    ]
