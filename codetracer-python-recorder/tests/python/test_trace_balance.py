import json
import importlib.util
import runpy
from pathlib import Path
from typing import List, Mapping

import pytest

import codetracer_python_recorder as codetracer

from codetracer_python_recorder.trace_balance import (
    TraceBalanceError,
    TraceBalanceResult,
    load_trace_events,
    summarize_trace_balance,
)


def _write_trace(tmp_path: Path, events: List[Mapping[str, object]]) -> Path:
    path = tmp_path / "trace.json"
    path.write_text(json.dumps(events))
    return path


def _load_cli() -> object:
    module_path = Path(__file__).parents[2] / "scripts" / "check_trace_balance.py"
    spec = importlib.util.spec_from_file_location("check_trace_balance", module_path)
    if spec is None or spec.loader is None:
        raise RuntimeError("Unable to load check_trace_balance.py module spec")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_summarize_trace_balance_identifies_balanced_trace() -> None:
    events = [{"Call": {}}, {"Return": {}}]
    result = summarize_trace_balance(events)

    assert isinstance(result, TraceBalanceResult)
    assert result.call_count == 1
    assert result.return_count == 1
    assert result.is_balanced
    assert result.delta == 0
    assert result.first_negative_index is None


def test_summarize_trace_balance_detects_missing_return() -> None:
    events = [{"Call": {}}, {"Call": {}}, {"Return": {}}]
    result = summarize_trace_balance(events)

    assert result.call_count == 2
    assert result.return_count == 1
    assert not result.is_balanced
    assert result.delta == 1
    assert result.first_negative_index is None


def test_summarize_trace_balance_detects_unmatched_return() -> None:
    events = [{"Return": {}}, {"Call": {}}]
    result = summarize_trace_balance(events)

    assert result.call_count == 1
    assert result.return_count == 1
    assert not result.is_balanced
    assert result.first_negative_index == 0


def test_load_trace_events_validates_structure(tmp_path: Path) -> None:
    trace_path = _write_trace(tmp_path, [{"Call": {}}])

    events = load_trace_events(trace_path)
    assert isinstance(events, list)
    assert events[0]["Call"] == {}


def test_load_trace_events_raises_on_non_array(tmp_path: Path) -> None:
    trace_path = tmp_path / "trace.json"
    trace_path.write_text("{}")

    with pytest.raises(TraceBalanceError):
        load_trace_events(trace_path)


def test_cli_returns_non_zero_on_unbalanced(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    trace_path = _write_trace(tmp_path, [{"Call": {}}, {"Return": {}}, {"Return": {}}])
    cli = _load_cli()

    exit_code = cli.main([str(trace_path)])
    captured = capsys.readouterr()

    assert exit_code == 1
    assert "Unbalanced trace detected." in captured.out
    assert "Unexpected 1 extra return event(s)." in captured.out


def test_cli_reports_success_for_balanced_trace(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    trace_path = _write_trace(tmp_path, [{"Call": {}}, {"Return": {}}])
    cli = _load_cli()

    exit_code = cli.main([str(trace_path)])
    captured = capsys.readouterr()

    assert exit_code == 0
    assert "Balanced trace" in captured.out


def test_activation_and_filter_skip_still_balances_trace(tmp_path: Path) -> None:
    script = tmp_path / "app.py"
    script.write_text(
        """
def side_effect():
    for _ in range(3):
        pass

if __name__ == "__main__":
    side_effect()
""",
        encoding="utf-8",
    )

    filter_file = tmp_path / "skip.toml"
    filter_file.write_text(
        """
[meta]
name = "skip-main"
version = 1

[scope]
default_exec = "trace"
default_value_action = "allow"

[[scope.rules]]
selector = "pkg:__main__"
exec = "skip"
value_default = "allow"
""",
        encoding="utf-8",
    )

    trace_dir = tmp_path / "trace"

    session = codetracer.start(
        trace_dir,
        format="json",
        start_on_enter=script,
        trace_filter=[filter_file],
    )
    try:
        runpy.run_path(str(script), run_name="__main__")
    finally:
        session.stop()

    events = load_trace_events(trace_dir / "trace.json")
    function_names: dict[int, str] = {}
    next_function_id = 0
    for event in events:
        payload = event.get("Function")
        if payload:
            function_names[next_function_id] = payload.get("name", "")
            next_function_id += 1

    toplevel_ids = {fid for fid, name in function_names.items() if name == "<toplevel>"}
    assert len(toplevel_ids) == 1, f"expected single toplevel function, saw {toplevel_ids}"

    toplevel_call_count = sum(
        1
        for event in events
        if "Call" in event and event["Call"].get("function_id") in toplevel_ids
    )
    assert toplevel_call_count == 1

    exit_returns = [
        event["Return"]
        for event in events
        if "Return" in event and event["Return"].get("return_value", {}).get("text") == "<exit>"
    ]
    assert len(exit_returns) == 1

    script_names = {name for name in function_names.values() if name not in {"<toplevel>"}}
    assert "side_effect" not in script_names
