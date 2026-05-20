"""CTFS trace inspection helpers for the Python test-suites.

Per ``codetracer-specs/Recorder-CLI-Conventions.md`` §4 the recorder is
CTFS-only: it emits a single ``<program>.ct`` container and never the
legacy ``trace.json`` / ``trace_metadata.json`` / ``trace_paths.json``
JSON sidecars (those were retired in commit ``efcfe28``, "#254 Phase 2").
The program / paths / function metadata that the sidecars used to carry
now lives inside the ``.ct`` container's ``meta.dat`` and per-stream
blocks.

This module is the test-side counterpart of that contract.  It records
a program through the public recorder API (or locates an
already-recorded container) and decodes it via the sibling-built
``ct-print`` tool from ``codetracer-trace-format-nim`` — the canonical
CTFS reader, exactly as ``test_cli_integration.py`` uses it.

Two decode depths are provided:

* :func:`ct_print_full` — ``ct-print --full``: every CBOR ``ValueRecord``
  (step vars, call args, return values, IO bytes) decoded to structured
  JSON.  Use for event-content assertions.
* :func:`ct_print_meta` — ``ct-print --meta-json``: ``meta.dat`` metadata
  (program, args, workdir, recorder, trace-filter provenance chain) plus
  interning-table / event counts, WITHOUT decoding the event streams.
  O(meta.dat)-fast even on multi-GB traces — use for metadata-only
  assertions (e.g. the pytest-integration suite).

:class:`ParsedCtfsTrace` adapts the decoded ``--full`` bundle to the
same shape the retired ``trace.json`` parser exposed (``paths``,
``functions`` with ``name``/``path_id``, ``calls``, ``call_records``,
``returns``, ``steps``, ``varnames``) so event-content tests migrate
without weakening a single assertion.
"""
from __future__ import annotations

import json
import os
import runpy
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Tuple

import codetracer_python_recorder as codetracer

__all__ = [
    "CTFS_MAGIC",
    "ct_print_binary",
    "ct_print_full",
    "ct_print_meta",
    "find_ct_file",
    "record_script",
    "ParsedCtfsTrace",
    "parse_ctfs_trace",
]


# CTFS magic bytes identifying a valid ``.ct`` trace container.
# See: codetracer-trace-format specification.
CTFS_MAGIC = bytes([0xC0, 0xDE, 0x72, 0xAC, 0xE2])


def ct_print_binary() -> Path:
    """Return the path to the ``ct-print`` binary used for CTFS decoding.

    Lookup order (mirrors ``test_cli_integration.py``):

    1. ``CT_PRINT`` environment variable — callers can point at a
       custom build.
    2. The sibling ``codetracer-trace-format-nim`` checkout under the
       workspace root.  ``Path(__file__).resolve().parents[4]`` walks
       up: ``ctfs.py`` → ``support/`` → ``python/`` → ``tests/`` →
       ``codetracer-python-recorder/`` (inner) → workspace root.
    """
    override = os.environ.get("CT_PRINT")
    if override:
        return Path(override)
    # ``ctfs.py`` → ``support/`` → ``python/`` → ``tests/`` →
    # ``codetracer-python-recorder/`` (inner) →
    # ``codetracer-python-recorder/`` (outer) → workspace root.
    nim_dir = Path(__file__).resolve().parents[5] / "codetracer-trace-format-nim"
    # The Nim build emits a bare ``ct-print`` on Unix and ``ct-print.exe``
    # on Windows.  Prefer whichever exists so the lookup is platform-correct.
    for name in ("ct-print.exe", "ct-print"):
        candidate = nim_dir / name
        if candidate.exists():
            return candidate
    # Fall back to the platform-default name so the descriptive
    # "binary missing" assertion fires at the call site when absent.
    return nim_dir / ("ct-print.exe" if os.name == "nt" else "ct-print")


def find_ct_file(trace_dir: Path) -> Path:
    """Locate the single CTFS ``.ct`` container in a recorded trace dir.

    The Nim writer names the produced container after the recorded
    program (e.g. ``program.ct``) rather than a literal ``trace.ct``,
    so callers must glob.  Raises ``AssertionError`` with a directory
    listing when no container is found, so failures stay diagnosable.
    It also asserts the legacy ``trace.json`` sidecar is absent — the
    recorder is CTFS-only.
    """
    assert trace_dir.is_dir(), f"trace directory missing: {trace_dir}"
    assert not (trace_dir / "trace.json").exists(), (
        "trace.json must not be produced — the recorder is CTFS-only"
    )
    ct_files = list(trace_dir.glob("*.ct"))
    assert ct_files, (
        f"No .ct files found in {trace_dir}; "
        f"contents: {list(trace_dir.iterdir())}"
    )
    assert len(ct_files) == 1, (
        f"expected exactly one .ct container in {trace_dir}, got {ct_files}"
    )
    ct_path = ct_files[0]
    # Verify the CTFS magic bytes — a truncated / non-CTFS file would
    # otherwise produce a confusing ct-print failure later.
    with open(ct_path, "rb") as handle:
        magic = handle.read(len(CTFS_MAGIC))
    assert magic == CTFS_MAGIC, (
        f"{ct_path} is not a valid CTFS container (magic={magic.hex()})"
    )
    return ct_path


def record_script(
    trace_dir: Path,
    script: Path,
    *,
    run_name: str = "__main__",
    start_on_enter: Path | None = None,
    configure_policy: Dict[str, Any] | None = None,
) -> Path:
    """Record *script* through the public recorder API into *trace_dir*.

    Starts the recorder (CTFS — the only format the recorder writes),
    executes *script* via :func:`runpy.run_path`, then flushes and stops
    the session.  Returns the path to the produced ``.ct`` container.

    ``start_on_enter`` restricts activation gating to the given file (as
    the retired ``trace.json`` tests did); it defaults to *script*.
    ``configure_policy`` is applied after ``stop()`` — used by the
    module-name policy test.
    """
    trace_dir.mkdir(parents=True, exist_ok=True)
    if start_on_enter is None:
        start_on_enter = script
    codetracer.start(trace_dir, start_on_enter=start_on_enter)
    try:
        runpy.run_path(str(script), run_name=run_name)
    finally:
        codetracer.flush()
        codetracer.stop()
        if configure_policy is not None:
            codetracer.configure_policy(**configure_policy)
    return find_ct_file(trace_dir)


def ct_print_full(ct_path: Path) -> Dict[str, Any]:
    """Decode *ct_path* with ``ct-print --full`` and return the bundle.

    The ``--full`` document decodes every CBOR ``ValueRecord`` to
    structured JSON.  Top-level keys: ``metadata``, ``paths``,
    ``functions``, ``varnames``, ``types``, ``counts``, ``events``.
    """
    binary = ct_print_binary()
    assert binary.exists(), f"ct-print binary missing at {binary}"
    result = subprocess.run(
        [str(binary), "--full", str(ct_path)],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def ct_print_meta(ct_path: Path) -> Dict[str, Any]:
    """Decode *ct_path* with ``ct-print --meta-json`` and return the doc.

    ``--meta-json`` materialises only ``meta.dat`` (program, args,
    workdir, recorder, ``trace_filter`` provenance chain) plus event /
    interning-table counts — it does NOT decode the per-event streams,
    so it stays fast even on the multi-MB traces a real ``pytest`` run
    produces.  Returns ``{"metadata": {...}, "counts": {...}}``.
    """
    binary = ct_print_binary()
    assert binary.exists(), f"ct-print binary missing at {binary}"
    result = subprocess.run(
        [str(binary), "--meta-json", str(ct_path)],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


@dataclass
class ParsedCtfsTrace:
    """Decoded CTFS trace adapted to the retired ``trace.json`` shape.

    Field meanings match the old ``ParsedTrace`` dataclass so the
    event-content tests assert exactly the facts they did before:

    * ``paths``         — interned source-path table.
    * ``functions``     — list indexed by ``function_id``; each entry is
      ``{"name": str, "path_id": int}``.  ``path_id`` is recovered from
      the step events that execute inside the function (the CTFS
      function table itself interns names only).
    * ``calls``         — ordered ``function_id`` of every ``call_entry``.
    * ``call_records``  — ordered raw ``call_entry`` payloads (carry
      decoded ``args``).
    * ``returns``       — ordered ``{"return_value": ...}`` payloads from
      every ``call_exit`` (and standalone return), preserving order.
    * ``steps``         — ordered ``(path_id, line)`` of every step.
    * ``varnames``      — interned variable-name table.
    """

    paths: List[str]
    functions: List[Dict[str, Any]]
    calls: List[int]
    call_records: List[Dict[str, Any]]
    returns: List[Dict[str, Any]]
    steps: List[Tuple[int, int]]
    varnames: List[str]
    events: List[Dict[str, Any]] = field(default_factory=list)


def parse_ctfs_trace(ct_path: Path) -> ParsedCtfsTrace:
    """Decode *ct_path* and adapt it to :class:`ParsedCtfsTrace`."""
    bundle = ct_print_full(ct_path)

    paths: List[str] = list(bundle["paths"])
    function_names: List[str] = list(bundle["functions"])
    varnames: List[str] = list(bundle["varnames"])
    events: List[Dict[str, Any]] = list(bundle["events"])

    # Recover each function's defining path_id from the step events that
    # run inside it.  The CTFS function table interns names only; step
    # events carry both function_id and path_id, which is how the writer
    # links a function to its source file.
    function_path_ids: Dict[int, int] = {}
    for event in events:
        if event.get("kind") == "step" and "function_id" in event and "path_id" in event:
            function_path_ids.setdefault(int(event["function_id"]), int(event["path_id"]))

    functions: List[Dict[str, Any]] = []
    for fid, name in enumerate(function_names):
        functions.append({"name": name, "path_id": function_path_ids.get(fid, -1)})

    calls: List[int] = []
    call_records: List[Dict[str, Any]] = []
    returns: List[Dict[str, Any]] = []
    steps: List[Tuple[int, int]] = []

    for event in events:
        kind = event.get("kind")
        if kind == "call_entry":
            calls.append(int(event["function_id"]))
            # ``call_entry.args`` already carries decoded ValueRecords;
            # expose it under the legacy ``args`` key so callers reuse
            # their arg-name / arg-value helpers unchanged.
            call_records.append(event)
        elif kind == "call_exit":
            returns.append({"return_value": event.get("return_value")})
        elif kind == "step":
            if "path_id" in event and "line" in event:
                steps.append((int(event["path_id"]), int(event["line"])))

    return ParsedCtfsTrace(
        paths=paths,
        functions=functions,
        calls=calls,
        call_records=call_records,
        returns=returns,
        steps=steps,
        varnames=varnames,
        events=events,
    )
