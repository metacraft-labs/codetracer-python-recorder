import json
import importlib.util
from pathlib import Path
from typing import List, Mapping

import pytest

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
