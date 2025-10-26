#!/usr/bin/env python3
"""
Prototype helper that shows which `sys.monitoring` events fire while a module
import runs to completion. The goal is to understand which callbacks the Rust
runtime should listen to in order to balance `<module>` start and finish events.

Usage examples:

    # Inspect a concrete file that is not on sys.path.
    python design-docs/prototypes/module_import_events.py \\
        --module-path /tmp/demo_module.py

    # Inspect an importable module that already lives on sys.path.
    python design-docs/prototypes/module_import_events.py \\
        --module json

Pass ``--include-lines`` to log `LINE` events as well, and ``--show-all`` to dump
events for every file that executed during the import (not just the target).
"""

from __future__ import annotations

import argparse
import contextlib
import importlib
import importlib.util
import itertools
import sys
from dataclasses import dataclass
from pathlib import Path
from types import CodeType, ModuleType
from typing import Dict, Iterable, Iterator, List, Optional, Tuple


_MODULE_ALIAS_COUNTER = itertools.count()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Record sys.monitoring events for a module import."
    )
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument(
        "--module",
        help="Importable module name (e.g., 'json'). Must already be on sys.path.",
    )
    target.add_argument(
        "--module-path",
        type=Path,
        help="Path to a Python file to import via spec_from_file_location.",
    )
    parser.add_argument(
        "--alias",
        help=(
            "Optional module alias when using --module-path. "
            "Defaults to an auto-generated unique name."
        ),
    )
    parser.add_argument(
        "--include-lines",
        action="store_true",
        help="Record LINE events in addition to start/finish callbacks.",
    )
    parser.add_argument(
        "--show-all",
        action="store_true",
        help="Print every recorded event, not just the ones for the target module.",
    )
    return parser.parse_args()


@dataclass
class EventRecord:
    index: int
    event: str
    code_name: str
    filename: str
    detail: str


class MonitoringProbe:
    """Capture sys.monitoring callbacks and store them as EventRecord entries."""

    def __init__(self) -> None:
        self._records: List[EventRecord] = []
        self._counter = itertools.count()

    def _add(self, event: str, code: CodeType, detail: str) -> None:
        self._records.append(
            EventRecord(
                index=next(self._counter),
                event=event,
                code_name=code.co_name,
                filename=code.co_filename,
                detail=detail,
            )
        )

    def on_py_start(self, code: CodeType, offset: int) -> None:
        self._add("PY_START", code, f"offset={offset}")

    def on_py_return(self, code: CodeType, offset: int, retval: object) -> None:
        self._add(
            "PY_RETURN",
            code,
            f"offset={offset}, retval={describe_value(retval)}",
        )

    def on_py_unwind(self, code: CodeType, offset: int, exc: object) -> None:
        self._add(
            "PY_UNWIND",
            code,
            f"offset={offset}, exception={describe_value(exc)}",
        )

    def on_py_yield(self, code: CodeType, offset: int, value: object) -> None:
        self._add(
            "PY_YIELD",
            code,
            f"offset={offset}, yielded={describe_value(value)}",
        )

    def on_py_resume(self, code: CodeType, offset: int) -> None:
        self._add("PY_RESUME", code, f"offset={offset}")

    def on_py_throw(self, code: CodeType, offset: int, exc: object) -> None:
        self._add(
            "PY_THROW",
            code,
            f"offset={offset}, exception={describe_value(exc)}",
        )

    def on_line(self, code: CodeType, line: int) -> None:
        self._add("LINE", code, f"line={line}")

    @property
    def records(self) -> List[EventRecord]:
        return list(self._records)

    def records_for(self, focus: Optional[Path]) -> List[EventRecord]:
        if focus is None:
            return list(self._records)
        focus_norm = normalize_path(focus)
        matches: List[EventRecord] = []
        for record in self._records:
            filename = record.filename
            if filename.startswith("<") and filename.endswith(">"):
                continue
            if normalize_path(filename) == focus_norm:
                matches.append(record)
        return matches


def describe_value(value: object, limit: int = 80) -> str:
    """Return a safe, trimmed repr for value payloads recorded in callbacks."""
    try:
        text = repr(value)
    except Exception as exc:  # pragma: no cover - prototyping aid
        text = f"<repr failed: {exc!r}>"
    if len(text) > limit:
        text = text[: limit - 3] + "..."
    return f"{type(value).__name__}={text}"


def normalize_path(value: Path | str) -> str:
    raw = str(value)
    if raw.startswith("<") and raw.endswith(">"):
        return raw
    return str(Path(raw).resolve())


def acquire_tool_id(name: str) -> Tuple[int, object]:
    """Reserve a monitoring tool id, trying the 6 CPython slots."""
    monitoring = sys.monitoring
    for candidate in range(6):
        try:
            monitoring.use_tool_id(candidate, name)
        except (RuntimeError, ValueError):
            continue
        return candidate, monitoring
    raise RuntimeError("all sys.monitoring tool ids are already in use")


@contextlib.contextmanager
def monitor_events(probe: MonitoringProbe, include_lines: bool) -> Iterator[None]:
    tool_id, monitoring = acquire_tool_id("module-import-events")
    events = monitoring.events
    callbacks: Dict[int, object] = {
        events.PY_START: probe.on_py_start,
        events.PY_RETURN: probe.on_py_return,
        events.PY_UNWIND: probe.on_py_unwind,
        events.PY_YIELD: probe.on_py_yield,
        events.PY_RESUME: probe.on_py_resume,
        events.PY_THROW: probe.on_py_throw,
    }
    if include_lines:
        callbacks[events.LINE] = probe.on_line

    mask = 0
    for event_id, handler in callbacks.items():
        monitoring.register_callback(tool_id, event_id, handler)
        mask |= event_id
    monitoring.set_events(tool_id, mask)

    try:
        yield
    finally:
        monitoring.set_events(tool_id, 0)
        for event_id in callbacks:
            monitoring.register_callback(tool_id, event_id, None)
        monitoring.free_tool_id(tool_id)


def import_target(args: argparse.Namespace) -> Tuple[ModuleType, Optional[Path]]:
    if args.module:
        module = importlib.import_module(args.module)
        file_attr = getattr(module, "__file__", None)
        module_path = Path(file_attr).resolve() if file_attr else None
        return module, module_path

    assert args.module_path is not None
    module_path = args.module_path.resolve()
    module_name = args.alias or f"import_probe_{module_path.stem}_{next(_MODULE_ALIAS_COUNTER)}"
    spec = importlib.util.spec_from_file_location(module_name, module_path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load module spec for {module_path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module, module_path


def print_report(
    probe: MonitoringProbe, focus: Optional[Path], show_all: bool, include_lines: bool
) -> None:
    total = len(probe.records)
    section = "=" * 72
    print(f"\n{section}")
    print("Module import monitoring session")
    print(f"Recorded events: {total}")
    print(f"Line events included: {include_lines}")
    if focus:
        print(f"Target path: {focus}")
    print(section)

    focus_records = probe.records_for(focus)
    title = (
        "Events for target module"
        if focus
        else "Events captured during import"
    )
    print(f"\n{title}:")
    _print_records(focus_records if focus else probe.records)

    if focus and show_all:
        print("\nAll recorded events (including other modules):")
        _print_records(probe.records)
    elif focus and len(focus_records) < total:
        remainder = total - len(focus_records)
        print(
            f"\nNOTE: {remainder} additional events came from other files "
            "during this import. Use --show-all to display them."
        )


def _print_records(records: Iterable[EventRecord]) -> None:
    records = list(records)
    if not records:
        print("  (no events)")
        return
    for record in records:
        path_display = record.filename
        if len(path_display) > 60:
            path_display = "..." + path_display[-57:]
        print(
            f"  #{record.index:03d} {record.event:<10} "
            f"{record.code_name:<20} {record.detail} [{path_display}]"
        )


def main() -> None:
    args = parse_args()
    probe = MonitoringProbe()
    focus_path: Optional[Path] = args.module_path.resolve() if args.module_path else None
    import_error: Optional[BaseException] = None

    with monitor_events(probe, include_lines=args.include_lines):
        try:
            _, loaded_path = import_target(args)
            if loaded_path is not None:
                focus_path = loaded_path
        except BaseException as exc:  # keep prototype noise visible
            import_error = exc

    print_report(probe, focus_path, args.show_all, args.include_lines)

    if import_error:
        raise import_error


if __name__ == "__main__":
    main()
