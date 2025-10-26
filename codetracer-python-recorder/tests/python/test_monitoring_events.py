import json
import runpy
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Tuple

import pytest

import codetracer_python_recorder as codetracer

from .support import ensure_trace_dir


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
    out_dir = ensure_trace_dir(tmp_path)

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
    out_dir = ensure_trace_dir(tmp_path)
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
    # String encoding must be stable for Python str values.
    # Enforce canonical kind String and exact text payload.
    assert v1.get("kind") == "String", f"Expected String encoding for str, got: {v1}"
    assert v1.get("text") == "x"


def test_all_argument_kinds_recorded_on_py_start(tmp_path: Path) -> None:
    # Arrange: write a script with a function using all Python argument kinds
    code = (
        "def g(p, /, q, *args, r, **kwargs):\n"
        "    # Touch values to ensure they're present in locals\n"
        "    return (p if p is not None else 0) + (q if q is not None else 0) + (r if r is not None else 0)\n\n"
        "if __name__ == '__main__':\n"
        "    g(10, 20, 30, 40, r=50, k=60)\n"
    )
    script = tmp_path / "script_args_kinds.py"
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

    # Locate g() function id
    assert str(script) in parsed.paths
    script_path_id = parsed.paths.index(str(script))
    g_fids = [i for i, f in enumerate(parsed.functions) if f["name"] == "g" and f["path_id"] == script_path_id]
    assert g_fids, "Expected function entry for g()"
    g_fid = g_fids[0]

    # Find the first Call to g()
    g_calls = [cr for cr in parsed.call_records if int(cr["function_id"]) == g_fid]
    assert g_calls, "Expected a recorded call to g()"
    call = g_calls[0]
    args = call.get("args", [])
    assert args, "Expected arguments for g() call"

    # Build name -> value mapping for convenience
    def arg_name(i: int) -> str:
        return parsed.varnames[int(args[i]["variable_id"])]

    def arg_value(i: int) -> Dict[str, Any]:
        return args[i]["value"]

    name_to_val: Dict[str, Dict[str, Any]] = {arg_name(i): arg_value(i) for i in range(len(args))}

    # Ensure we captured all kinds by name
    for expected in ("p", "q", "args", "r", "kwargs"):
        assert expected in name_to_val, f"Missing argument kind: {expected} in {list(name_to_val.keys())}"

    # Validate some concrete values where encoding is unambiguous
    assert name_to_val["p"].get("kind") == "Int" and int(name_to_val["p"].get("i")) == 10
    assert name_to_val["q"].get("kind") == "Int" and int(name_to_val["q"].get("i")) == 20
    assert name_to_val["r"].get("kind") == "Int" and int(name_to_val["r"].get("i")) == 50

    # Varargs may be encoded as a structured sequence or as a Raw string; accept both
    varargs_val = name_to_val["args"]
    kind = varargs_val.get("kind")
    assert kind in ("Sequence", "Raw", "Tuple"), f"Unexpected *args encoding kind: {kind}"
    if kind in ("Sequence", "Tuple"):
        elems = varargs_val.get("elements")
        assert isinstance(elems, list) and len(elems) == 2
        assert elems[0].get("kind") == "Int" and int(elems[0].get("i")) == 30
        assert elems[1].get("kind") == "Int" and int(elems[1].get("i")) == 40
    else:
        # Raw textual fallback
        r = varargs_val.get("r", "")
        assert "30" in r and "40" in r

    # Kwargs must be encoded structurally as a sequence of (key, value) tuples
    kwargs_val = name_to_val["kwargs"]
    assert kwargs_val.get("kind") == "Sequence", f"Expected structured kwargs encoding, got: {kwargs_val}"
    elements = kwargs_val.get("elements")
    assert isinstance(elements, list) and len(elements) == 1, f"Expected single kwargs pair, got: {elements}"
    pair = elements[0]
    assert pair.get("kind") == "Tuple", f"Expected key/value tuple, got: {pair}"
    kv = pair.get("elements")
    assert isinstance(kv, list) and len(kv) == 2, f"Expected 2-tuple for kwargs item, got: {kv}"
    key_rec, val_rec = kv[0], kv[1]
    assert key_rec.get("kind") == "String" and key_rec.get("text") == "k", f"Unexpected kwargs key encoding: {key_rec}"
    assert val_rec.get("kind") == "Int" and int(val_rec.get("i")) == 60, f"Unexpected kwargs value encoding: {val_rec}"


def test_generator_yield_resume_events_are_balanced(tmp_path: Path) -> None:
    code = (
        "def ticker():\n"
        "    yield 'tick-one'\n"
        "    yield 'tick-two'\n"
        "    return 'tick-complete'\n\n"
        "if __name__ == '__main__':\n"
        "    g = ticker()\n"
        "    try:\n"
        "        next(g)\n"
        "        next(g)\n"
        "        next(g)\n"
        "    except StopIteration:\n"
        "        pass\n"
    )
    script = tmp_path / "script_generator.py"
    script.write_text(code)

    out_dir = ensure_trace_dir(tmp_path)
    session = codetracer.start(out_dir, format=codetracer.TRACE_JSON, start_on_enter=script)
    try:
        runpy.run_path(str(script), run_name="__main__")
    finally:
        codetracer.flush()
        codetracer.stop()

    parsed = _parse_trace(out_dir)
    assert str(script) in parsed.paths
    script_path_id = parsed.paths.index(str(script))

    ticker_fids = [
        i
        for i, f in enumerate(parsed.functions)
        if f["name"] == "ticker" and f["path_id"] == script_path_id
    ]
    assert ticker_fids, "Expected ticker() to be registered"
    ticker_fid = ticker_fids[0]

    ticker_calls = [
        call for call in parsed.call_records if int(call["function_id"]) == ticker_fid
    ]
    assert (
        len(ticker_calls) == 3
    ), f"Expected three call edges for ticker(), saw {len(ticker_calls)}"

    string_returns = [
        rv_value.get("text")
        for rv in parsed.returns
        if (rv_value := rv.get("return_value")) and rv_value.get("kind") == "String"
    ]
    seq = [value for value in string_returns if value in {"tick-one", "tick-two", "tick-complete"}]
    assert seq == ["tick-one", "tick-two", "tick-complete"], f"Unexpected return order: {seq}"


def test_generator_throw_records_exception_argument(tmp_path: Path) -> None:
    code = (
        "def worker():\n"
        "    try:\n"
        "        yield 'ready'\n"
        "    except RuntimeError as err:\n"
        "        return f'caught {err}'\n\n"
        "if __name__ == '__main__':\n"
        "    g = worker()\n"
        "    next(g)\n"
        "    try:\n"
        "        g.throw(RuntimeError('boom'))\n"
        "    except StopIteration:\n"
        "        pass\n"
    )
    script = tmp_path / "script_throw.py"
    script.write_text(code)

    out_dir = ensure_trace_dir(tmp_path)
    session = codetracer.start(out_dir, format=codetracer.TRACE_JSON, start_on_enter=script)
    try:
        runpy.run_path(str(script), run_name="__main__")
    finally:
        codetracer.flush()
        codetracer.stop()

    parsed = _parse_trace(out_dir)
    assert str(script) in parsed.paths
    script_path_id = parsed.paths.index(str(script))

    worker_fids = [
        i
        for i, f in enumerate(parsed.functions)
        if f["name"] == "worker" and f["path_id"] == script_path_id
    ]
    assert worker_fids, "Expected worker() to be registered"
    worker_fid = worker_fids[0]

    worker_calls = [
        call for call in parsed.call_records if int(call["function_id"]) == worker_fid
    ]
    assert len(worker_calls) >= 2, "Expected multiple call records for worker()"

    def arg_name(arg: Dict[str, Any]) -> str:
        return parsed.varnames[int(arg["variable_id"])]

    exception_calls = [
        call
        for call in worker_calls
        if any(arg_name(arg) == "exception" for arg in call.get("args", []))
    ]
    assert exception_calls, "Expected PY_THROW to register an 'exception' argument"

    assert any(
        (rv_value := rv.get("return_value"))
        and rv_value.get("kind") == "String"
        and rv_value.get("text") == "caught boom"
        for rv in parsed.returns
    ), "Expected final return value for worker() to capture caught exception message"
