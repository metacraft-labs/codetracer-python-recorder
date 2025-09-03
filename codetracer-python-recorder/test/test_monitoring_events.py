import json
import runpy
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Tuple

import pytest

import codetracer_python_recorder as codetracer


@dataclass
class ParsedTrace:
    paths: List[str]
    functions: List[Dict[str, Any]]  # index is function_id
    calls: List[int]  # sequence of function_id values
    returns: List[Dict[str, Any]]  # raw Return payloads (order preserved)
    steps: List[Tuple[int, int]]  # (path_id, line)
    varnames: List[str]  # index is variable_id
    call_records: List[Dict[str, Any]]  # raw Call payloads (order preserved)


def _parse_trace(out_dir: Path) -> ParsedTrace:
    events_path = out_dir / "trace.json"
    paths_path = out_dir / "trace_paths.json"

    events = json.loads(events_path.read_text())
    paths: List[str] = json.loads(paths_path.read_text())

    functions: List[Dict[str, Any]] = []
    calls: List[int] = []
    returns: List[Dict[str, Any]] = []
    steps: List[Tuple[int, int]] = []
    varnames: List[str] = []
    call_records: List[Dict[str, Any]] = []

    for item in events:
        if "Function" in item:
            functions.append(item["Function"])
        elif "Call" in item:
            calls.append(int(item["Call"]["function_id"]))
            call_records.append(item["Call"])  # keep for arg assertions
        elif "VariableName" in item:
            varnames.append(item["VariableName"])  # index is VariableId
        elif "Return" in item:
            returns.append(item["Return"])  # keep raw payload for value checks
        elif "Step" in item:
            s = item["Step"]
            steps.append((int(s["path_id"]), int(s["line"])))

    return ParsedTrace(
        paths=paths,
        functions=functions,
        calls=calls,
        returns=returns,
        steps=steps,
        varnames=varnames,
        call_records=call_records,
    )


def _write_script(tmp: Path) -> Path:
    # Keep lines compact and predictable to assert step line numbers
    code = (
        "# simple script\n\n"
        "def foo():\n"
        "    x = 1\n"
        "    y = 2\n"
        "    return x + y\n\n"
        "if __name__ == '__main__':\n"
        "    r = foo()\n"
        "    print(r)\n"
    )
    p = tmp / "script.py"
    p.write_text(code)
    return p


def test_py_start_line_and_return_events_are_recorded(tmp_path: Path) -> None:
    # Arrange: create a script and start tracing with activation restricted to that file
    script = _write_script(tmp_path)
    out_dir = tmp_path / "trace_out"
    out_dir.mkdir()

    session = codetracer.start(out_dir, format=codetracer.TRACE_JSON, start_on_enter=script)

    try:
        # Act: execute the script as __main__ under tracing
        runpy.run_path(str(script), run_name="__main__")
    finally:
        # Ensure files are flushed and tracer is stopped even on error
        codetracer.flush()
        codetracer.stop()

    # Assert: expected files exist and contain valid JSON
    assert (out_dir / "trace.json").exists()
    assert (out_dir / "trace_metadata.json").exists()
    assert (out_dir / "trace_paths.json").exists()

    parsed = _parse_trace(out_dir)

    # The script path must be present (activation gating starts there, but
    # other helper modules like codecs may also appear during execution).
    assert str(script) in parsed.paths
    script_path_id = parsed.paths.index(str(script))

    # One function named 'foo' should be registered for the script
    foo_fids = [i for i, f in enumerate(parsed.functions) if f["name"] == "foo" and f["path_id"] == script_path_id]
    assert foo_fids, "Expected function entry for foo()"
    foo_fid = foo_fids[0]

    # A call to foo() must be present (PY_START) and matched by a later return (PY_RETURN)
    assert foo_fid in parsed.calls, "Expected a call to foo() to be recorded"

    # Returns are emitted in order; the first Return in this script should be the result of foo()
    # and carry the concrete integer value 3 encoded by the writer
    first_return = parsed.returns[0]
    rv = first_return.get("return_value", {})
    assert rv.get("kind") == "Int" and rv.get("i") == 3

    # LINE events: confirm that the key lines within foo() were stepped
    # Compute concrete line numbers by scanning the file content
    lines = script.read_text().splitlines()
    want_lines = {
        next(i + 1 for i, t in enumerate(lines) if t.strip() == "x = 1"),
        next(i + 1 for i, t in enumerate(lines) if t.strip() == "y = 2"),
        next(i + 1 for i, t in enumerate(lines) if t.strip() == "return x + y"),
    }
    seen_lines = {ln for pid, ln in parsed.steps if pid == script_path_id}
    assert want_lines.issubset(seen_lines), f"Missing expected step lines: {want_lines - seen_lines}"


def test_start_while_active_raises(tmp_path: Path) -> None:
    out_dir = tmp_path / "trace_out"
    out_dir.mkdir()
    session = codetracer.start(out_dir, format=codetracer.TRACE_JSON)
    try:
        with pytest.raises(RuntimeError):
            codetracer.start(out_dir, format=codetracer.TRACE_JSON)
    finally:
        codetracer.stop()


def test_call_arguments_recorded_on_py_start(tmp_path: Path) -> None:
    # Arrange: write a simple script with a function that accepts two args
    code = (
        "def foo(a, b):\n"
        "    return a if len(str(b)) > 0 else 0\n\n"
        "if __name__ == '__main__':\n"
        "    foo(1, 'x')\n"
    )
    script = tmp_path / "script_args.py"
    script.write_text(code)

    out_dir = tmp_path / "trace_out"
    out_dir.mkdir()

    session = codetracer.start(out_dir, format=codetracer.TRACE_JSON, start_on_enter=script)
    try:
        runpy.run_path(str(script), run_name="__main__")
    finally:
        codetracer.flush()
        codetracer.stop()

    parsed = _parse_trace(out_dir)

    # Locate foo() function id in this script
    assert str(script) in parsed.paths
    script_path_id = parsed.paths.index(str(script))
    foo_fids = [i for i, f in enumerate(parsed.functions) if f["name"] == "foo" and f["path_id"] == script_path_id]
    assert foo_fids, "Expected function entry for foo()"
    foo_fid = foo_fids[0]

    # Find the first Call to foo() and assert it carries two args with correct names/values
    foo_calls = [cr for cr in parsed.call_records if int(cr["function_id"]) == foo_fid]
    assert foo_calls, "Expected a recorded call to foo()"
    call = foo_calls[0]
    args = call.get("args", [])
    assert len(args) == 2, f"Expected 2 args, got: {args}"

    def arg_name(i: int) -> str:
        return parsed.varnames[int(args[i]["variable_id"])]

    def arg_value(i: int) -> Dict[str, Any]:
        return args[i]["value"]

    # Validate names and values
    assert arg_name(0) == "a"
    assert arg_value(0).get("kind") == "Int" and int(arg_value(0).get("i")) == 1
    assert arg_name(1) == "b"
    v1 = arg_value(1)
    assert v1.get("kind") in ("String", "Raw")  # backend may encode as String or Raw
    if v1.get("kind") == "String":
        assert v1.get("text") == "x"
    else:
        assert v1.get("r") == "x"
