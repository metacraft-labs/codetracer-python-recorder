"""Cross-check the production recorder against the pure-Python test oracle.

``codetracer-pure-python-recorder`` is a test oracle, not a production
recorder: it writes JSON (``trace.json``) that CodeTracer cannot open. Its
only purpose is this protocol:

1. run a program through the pure recorder -> ``trace.json``;
2. run the same program through the production recorder -> ``<prog>.ct``;
3. convert the ``.ct`` with ``ct print`` (``ct-print --full``);
4. project both onto the facts the two recorders must agree on and assert
   the projections are equal.

The projection keeps, per call frame of the recorded program: the function
name and its arguments, the sequence of executed lines, the return value,
and the values of the frame's local variables at each executed line. It
drops what the two implementations legitimately record differently:

* the outermost synthetic frame (``<top-level>`` vs ``<toplevel>``) and the
  module frame's name (``<module>`` vs ``<__main__>``);
* consecutive repeats of the same line within one frame (the pure recorder
  emits an extra step at every ``return``);
* the function-definition line at call entry, which the production recorder
  records as the call's first step: the projection adds it to the pure side;
* variables the production recorder captures but the pure one never does
  (dunder names, functions, module globals seen from a function frame).
  Every local the pure recorder captured must be present in the production
  recorder's step with an equal value; extra production variables are
  tolerated.
* when a line calls a function, ``ct-print`` lists the callee's argument
  values after the caller step's own variables (so a step on
  ``rest = factorial(n - 1)`` shows ``n`` twice, 5 and then 4). The
  projection keeps the first value of each name, the frame's own.

No mocks: both recorders run for real, in subprocesses, on real files, and
``ct-print`` is the real CTFS decoder.

The comparison refuses to be vacuous: before comparing, both sides must
have recorded at least one call of a program function, several executed
lines, a non-None return value and compared local values.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import pytest

from .support.ctfs import ct_print_full, find_ct_file

TESTS_DIR = Path(__file__).resolve().parent
REPO_ROOT = TESTS_DIR.parents[2]
PURE_RECORDER_DIR = REPO_ROOT / "codetracer-pure-python-recorder"
PURE_TRACE_SCRIPT = PURE_RECORDER_DIR / "src" / "trace.py"

PROGRAMS = sorted(
    list((PURE_RECORDER_DIR / "tests" / "programs").glob("*.py"))
    + list((TESTS_DIR / "oracle_programs").glob("*.py"))
)

OUTER_FRAMES = {"<top-level>", "<toplevel>"}
MODULE_FRAMES = {"<module>", "<__main__>"}


# --------------------------------------------------------------- projection
def _value(v: Optional[Dict[str, Any]]) -> Any:
    """Reduce a recorded value to a recorder-independent form."""
    if v is None:
        return ("Missing",)
    kind = v.get("kind")
    if kind == "Int":
        return ("Int", v["i"])
    if kind == "String":
        return ("String", v["text"])
    if kind == "Bool":
        return ("Bool", v["b"])
    if kind == "None":
        return ("None",)
    if kind == "Sequence":
        return ("Sequence", tuple(_value(e) for e in v.get("elements", [])))
    return ("Other", kind, v.get("r"))


def _comparable_local(name: str, value: Any) -> bool:
    return not name.startswith("__") and value[0] != "Other"


@dataclass
class Projection:
    """A recorder-independent view of one recording."""

    flow: List[Tuple[Any, ...]] = field(default_factory=list)
    # locals[i] holds the variables at flow[i] when flow[i] is a line.
    locals: List[Dict[str, Any]] = field(default_factory=list)
    stdout: List[str] = field(default_factory=list)

    def emit(self, entry: Tuple[Any, ...], local_vars: Optional[Dict[str, Any]] = None) -> None:
        self.flow.append(entry)
        self.locals.append(local_vars or {})


class _Frames:
    """Per-frame bookkeeping shared by both projections."""

    def __init__(self, out: Projection) -> None:
        self.out = out
        self.stack: List[Dict[str, Any]] = []

    @staticmethod
    def _name(name: str) -> str:
        return "<module>" if name in MODULE_FRAMES else name

    def call(self, name: str, args: Tuple[Any, ...]) -> None:
        visible = name not in OUTER_FRAMES
        self.stack.append({"name": self._name(name), "visible": visible, "last": None})
        if visible:
            self.out.emit(("call", self._name(name), args))

    def line(self, line: int, local_vars: Dict[str, Any]) -> None:
        if not self.stack:
            return
        frame = self.stack[-1]
        if not frame["visible"] or frame["last"] == line:
            return
        frame["last"] = line
        self.out.emit(("line", line), local_vars)

    def ret(self, value: Any) -> None:
        if not self.stack:
            return
        frame = self.stack.pop()
        if frame["visible"]:
            self.out.emit(("return", frame["name"], value))


def project_pure(events: List[Dict[str, Any]]) -> Projection:
    out = Projection()
    frames = _Frames(out)
    functions: List[Dict[str, Any]] = []
    varnames: List[str] = []
    current: Dict[str, Any] = {}
    pending_line: Optional[int] = None

    def flush_line() -> None:
        nonlocal pending_line, current
        if pending_line is not None:
            frames.line(pending_line, current)
        pending_line = None
        current = {}

    for event in events:
        ((kind, payload),) = event.items()
        if kind == "Function":
            functions.append(payload)
        elif kind == "VariableName":
            varnames.append(payload)
        elif kind == "Call":
            flush_line()
            fn = functions[payload["function_id"]]
            args = tuple(
                sorted((varnames[a["variable_id"]], _value(a["value"])) for a in payload["args"])
            )
            frames.call(fn["name"], args)
            # The production recorder's first step in a call is the
            # function's definition line; the pure recorder has no such step.
            if fn["name"] not in OUTER_FRAMES:
                frames.line(fn["line"], {})
        elif kind == "Step":
            flush_line()
            pending_line = payload["line"]
        elif kind == "Value":
            name = varnames[payload["variable_id"]]
            if name != "<return_value>":
                current[name] = _value(payload["value"])
        elif kind == "Return":
            flush_line()
            frames.ret(_value(payload["return_value"]))
        elif kind == "Event":
            out.stdout.append(payload["content"])
    flush_line()
    return out


def project_production(bundle: Dict[str, Any]) -> Projection:
    out = Projection()
    frames = _Frames(out)
    text: List[str] = []
    for event in bundle["events"]:
        kind = event.get("kind")
        if kind == "call_entry":
            args = tuple(
                sorted((a["varname"], _value(a.get("value"))) for a in event.get("args", []))
            )
            frames.call(event["function"], args)
        elif kind == "step":
            # A step's own snapshot comes first. When the step is followed
            # by a call, ``ct-print`` also lists the callee's argument values
            # under the caller's step (see the module header); keep the first
            # value of each name, which is the frame's own.
            local_vars: Dict[str, Any] = {}
            for v in event.get("vars", []):
                local_vars.setdefault(v["varname"], _value(v.get("value")))
            frames.line(int(event["line"]), local_vars)
        elif kind == "call_exit":
            frames.ret(_value(event.get("return_value")))
        elif kind == "io" and '"stream":"stdout"' in event.get("metadata", ""):
            text.append(event.get("text", ""))
    out.stdout = "".join(text).splitlines()
    return out


# ---------------------------------------------------------------- recording
def record_pure(program: Path, workdir: Path) -> List[Dict[str, Any]]:
    workdir.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [sys.executable, str(PURE_TRACE_SCRIPT), str(program)],
        cwd=workdir,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads((workdir / "trace.json").read_text(encoding="utf-8"))


def record_production(program: Path, out_dir: Path) -> Dict[str, Any]:
    env = os.environ.copy()
    env.pop("CODETRACER_TRACE_FILTER", None)
    subprocess.run(
        [
            sys.executable,
            "-m",
            "codetracer_python_recorder",
            "--out-dir",
            str(out_dir),
            str(program),
        ],
        cwd=program.parent,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )
    return ct_print_full(find_ct_file(out_dir))


# --------------------------------------------------------------- comparison
def compare(pure: Projection, production: Projection) -> Tuple[List[str], int]:
    """Return (mismatches, number of local values compared)."""
    problems: List[str] = []
    if pure.flow != production.flow:
        for i, (a, b) in enumerate(zip(pure.flow, production.flow)):
            if a != b:
                problems.append(f"control flow diverges at #{i}: pure={a!r} production={b!r}")
                break
        else:
            problems.append(
                f"control flow lengths differ: pure={len(pure.flow)} production={len(production.flow)}"
            )
        return problems, 0

    compared = 0
    for i, entry in enumerate(pure.flow):
        if entry[0] != "line":
            continue
        for name, value in pure.locals[i].items():
            if not _comparable_local(name, value):
                continue
            compared += 1
            actual = production.locals[i].get(name, ("Missing",))
            if actual != value:
                problems.append(
                    f"line {entry[1]} (#{i}): {name} = {value!r} in pure, {actual!r} in production"
                )
    if pure.stdout != production.stdout:
        problems.append(f"stdout differs: pure={pure.stdout!r} production={production.stdout!r}")
    return problems, compared


def assert_not_vacuous(label: str, p: Projection) -> None:
    calls = [e for e in p.flow if e[0] == "call" and e[1] != "<module>"]
    lines = [e for e in p.flow if e[0] == "line"]
    returns = [e for e in p.flow if e[0] == "return" and e[2] != ("None",)]
    assert calls, f"{label}: no call of a program function was recorded"
    assert len(lines) >= 4, f"{label}: only {len(lines)} executed lines were recorded"
    assert returns, f"{label}: no non-None return value was recorded"
    assert any(p.locals[i] for i, e in enumerate(p.flow) if e[0] == "line"), (
        f"{label}: no local variable values were recorded"
    )


def _record_both(program: Path, tmp_path: Path) -> Tuple[Projection, Projection]:
    work = tmp_path / "src"
    work.mkdir()
    local_program = work / program.name
    shutil.copy(program, local_program)
    pure = project_pure(record_pure(local_program, tmp_path / "pure"))
    production = project_production(record_production(local_program, tmp_path / "ct"))
    return pure, production


# -------------------------------------------------------------------- tests
def test_oracle_programs_are_present() -> None:
    assert len(PROGRAMS) >= 5, f"expected the oracle program set, found {PROGRAMS}"


@pytest.mark.parametrize("program", PROGRAMS, ids=lambda p: p.stem)
def test_production_recorder_agrees_with_pure_oracle(program: Path, tmp_path: Path) -> None:
    pure, production = _record_both(program, tmp_path)

    assert_not_vacuous("pure oracle", pure)
    assert_not_vacuous("production recorder", production)

    problems, compared = compare(pure, production)
    assert not problems, "production recorder disagrees with the pure oracle:\n" + "\n".join(
        problems
    )
    assert compared > 0, "no local variable value was compared"


def test_comparison_detects_a_wrong_value(tmp_path: Path) -> None:
    """The comparison must fail when the recorders disagree on a value."""
    program = TESTS_DIR / "oracle_programs" / "recursion.py"
    pure, production = _record_both(program, tmp_path)

    for i, entry in enumerate(production.flow):
        if entry[0] == "line" and production.locals[i].get("n") == ("Int", 3):
            production.locals[i]["n"] = ("Int", 4)
            break
    else:
        pytest.fail("the recursion program never recorded n == 3")
    problems, _ = compare(pure, production)
    assert any("n = " in p for p in problems), problems


def test_comparison_detects_a_wrong_line(tmp_path: Path) -> None:
    """The comparison must fail when the recorders disagree on control flow."""
    program = TESTS_DIR / "oracle_programs" / "nested_calls.py"
    pure, production = _record_both(program, tmp_path)

    i = next(i for i, e in enumerate(production.flow) if e[0] == "line" and i > 3)
    production.flow[i] = ("line", production.flow[i][1] + 100)
    problems, _ = compare(pure, production)
    assert problems and "control flow diverges" in problems[0], problems
