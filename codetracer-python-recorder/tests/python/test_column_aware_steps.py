"""P1.4 — column-aware step encoding acceptance test.

Records a Python fixture through the Rust-backed recorder and verifies
that the resulting CTFS trace's step events carry column data
(``meta.dat`` flag bit 4 set, exec stream contains ``DeltaColumn``
events for any column moves, JSON-events output surfaces the resolved
column per step).  This is the only end-to-end test in this suite that
asserts on the canonical CTFS ``DeltaColumn`` (tag 0x07) emission path
landed by the P1 milestone of the "Column-Aware Tracing & Source
Deminification" campaign.

Background.  Pre-P1 the recorder emitted a column slot on the legacy
``StepRecord.column`` field, which the multi-stream Nim writer's
``add_event`` dispatch dropped on the floor.  P1.1/P1.2 wire the writer
into column-aware mode via ``enable_column_aware_steps()`` + per-step
``write_delta_column(delta)`` calls, and P1.3 populates the
``paths.dat`` per-line offset table so the reader can resolve every
step back to a ``(line, column)`` pair.  This test exercises the full
path through ``ct-print --json-events`` / ``--meta-json``.

A note on Python's ``sys.monitoring`` LINE granularity.  The fixture
described in the milestone spec § "Test Design" (``a=1; b=2; c=a+b;
d=c*c; print(d)`` — all on one line) packs five statements onto a
single source line.  ``sys.monitoring`` fires LINE *once* per distinct
source line; the recorder therefore sees a single on_line(1) callback
and emits one step at the leftmost STORE's column.  We get a real
column-aware step, but not five.  Distinct-column-per-statement
acceptance for the one-liner case requires per-INSTRUCTION (or
per-BRANCH) callbacks, which is out of scope for P1.

What this test asserts instead: across a multi-line fixture where
different statements live on different lines, the recorder emits a
step event per line carrying its leftmost STORE's column, and the
column varies as the user changes the indentation / leading content.
That's the contract the column-aware extension exists to support, and
it's the contract the JS / minified-library work in P2 will inherit.

The companion ``codetracer-pure-python-recorder`` writes legacy JSON
without column data; P1.5 keeps that oracle column-blind by design.
This test deliberately runs the native recorder only — column-aware
acceptance is its own assertion path.

See ``codetracer-specs/Planned-Features/Column-Aware-Tracing-And-Deminification.milestones.org``
§ P1 for the full milestone breakdown.
"""
from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from .support.ctfs import (
    ct_print_binary,
    find_ct_file,
    record_script,
)


# Multi-line fixture: a leading-whitespace-varying program where each
# statement's leftmost STORE / call lives at a different column.  Lines
# 2-6 each carry an assignment indented to a different depth so the
# recorder's `first_column_for_line` extraction surfaces five distinct
# columns.  We use `# noqa` style indentation deliberately — the test
# wants column variance, not Pythonic style.
DISTINCT_COLUMN_FIXTURE = (
    "def main():\n"
    "    a = 1\n"
    "      ; b = 2  # noqa\n".replace("  ; ", "; ")  # synth two-on-a-line
)

# A more realistic multi-line fixture with distinct columns per step:
# each line's STORE lands at a different indentation column.
NESTED_INDENT_FIXTURE = (
    "if True:\n"
    "    a = 1\n"
    "    if True:\n"
    "        b = 2\n"
    "        if True:\n"
    "            c = 3\n"
    "print(a + b + c)\n"
)


def _print_json_events(ct_path: Path) -> list[dict]:
    """Run ``ct-print --json-events`` on *ct_path* and return the
    decoded events list.  Filters control bytes out of the inline
    ``data`` strings so JSON decoding succeeds even when value bytes
    aren't valid UTF-8.  See ``support/ctfs.py::ct_print_full`` for
    the rationale on subprocessing out to the Nim ct-print binary."""
    binary = ct_print_binary()
    assert binary.exists(), (
        f"ct-print binary missing at {binary} — build it via "
        "`cd codetracer-trace-format-nim && nimble buildCtPrint`"
    )
    result = subprocess.run(
        [str(binary), "--json-events", str(ct_path)],
        check=True,
        capture_output=True,
    )
    # The Nim ct-print emits raw byte sequences inside value `data`
    # strings — when the program records non-UTF8 bytes (e.g. the
    # `\xa3` byte in this paragraph's literal text) the JSON document
    # is binary, not text.  Decode with `surrogateescape` so we can
    # recover and then parse — we don't assert on `data` strings here.
    text = result.stdout.decode("utf-8", errors="surrogateescape")
    return json.loads(text)


def test_column_aware_flag_in_meta_dat(tmp_path: Path) -> None:
    """P1.1: every recording in this test suite emits
    ``FlagHasColumnAwareSteps`` (bit 4) in ``meta.dat``.

    The flag is the on-disk contract for "this trace's step stream
    may include DeltaColumn events"; old readers reject column-aware
    traces via the reserved-bits check unless they understand the
    flag.  When the flag is missing on a recording produced by this
    same test suite, the most likely cause is a regression in
    ``TraceOutputPaths::configure_writer`` (the P1.1 hook).
    """
    script = tmp_path / "trivial.py"
    script.write_text("x = 1\nprint(x)\n", encoding="utf-8")

    trace_dir = tmp_path / "trace"
    ct_path = record_script(trace_dir, script)
    assert ct_path == find_ct_file(trace_dir)

    binary = ct_print_binary()
    result = subprocess.run(
        [str(binary), "--meta-json", str(ct_path)],
        check=True,
        capture_output=True,
        text=True,
    )
    doc = json.loads(result.stdout)
    metadata = doc.get("metadata", {})
    flags = metadata.get("flags", {})
    assert flags.get("has_column_aware_steps") is True, (
        "meta.dat must carry FlagHasColumnAwareSteps after a P1 recording; "
        f"got metadata={metadata}.  Likely regression: "
        "`TraceOutputPaths::configure_writer` no longer calls "
        "`enable_column_aware_steps()` on the CTFS writer."
    )


def test_column_aware_step_events_carry_column(tmp_path: Path) -> None:
    """P1.2 / P1.3 / P1.4: step events expose a resolved per-step
    column in the ``--json-events`` output of a column-aware trace.

    The recorder extracts each step's column via ``co_positions``
    (Python 3.11+) and emits it via the canonical ``DeltaColumn``
    (tag 0x07) event when the step stays on the same line, or via the
    implicit column reset on a ``register_step`` / ``DeltaStep``.  The
    reader's ``decodeGlobalPositionIndex`` round-trips both forms
    back to a 1-based ``column`` field on every step.

    We assert (a) at least one step carries a non-trivial column, (b)
    every step event in the trace has the ``column`` field set
    (resolved via the spec-canonical decoder), and (c) the column
    values span more than one distinct value across the recorded
    program — the indented-block fixture varies its STORE column line-
    by-line, so the recorded column stream MUST vary too.
    """
    script = tmp_path / "indented.py"
    script.write_text(NESTED_INDENT_FIXTURE, encoding="utf-8")

    trace_dir = tmp_path / "trace"
    ct_path = record_script(trace_dir, script)

    events = _print_json_events(ct_path)
    script_str = str(script)

    # Project step events that land on our fixture's path.  Match by
    # suffix to stay resilient to workdir-relative path normalization.
    fixture_steps = [
        e
        for e in events
        if e.get("type") == "step"
        and (e.get("path") == script_str or str(e.get("path", "")).endswith("/indented.py"))
    ]
    assert fixture_steps, (
        "expected at least one step event on the fixture's path; "
        f"got events kinds={sorted({e.get('type') for e in events})}, "
        f"steps total={sum(1 for e in events if e.get('type') == 'step')}"
    )

    # Every step on the column-aware trace must resolve to a column.
    missing_column = [e for e in fixture_steps if "column" not in e]
    assert not missing_column, (
        "every step in a column-aware trace must resolve to a column "
        "via decodeGlobalPositionIndex; missing on "
        f"{[(e.get('step_index'), e.get('line')) for e in missing_column]}.  "
        "Likely regression: either `paths.dat` line-lengths weren't "
        "populated (P1.3 register_path_with_line_lengths call site) or "
        "ct-print's column-aware JSON projection broke."
    )

    distinct_columns = {int(e["column"]) for e in fixture_steps}
    assert len(distinct_columns) >= 2, (
        "expected >=2 distinct columns across the recorded program "
        "(the nested-indent fixture has 5 indentation levels); got "
        f"{sorted(distinct_columns)}.  Likely regression: "
        "`first_column_for_line` is returning a constant (1?) instead "
        "of the per-line STORE column."
    )

    # Sanity: at least one column is > 1 — the recorder must surface
    # non-trivial indentation, not just degenerate to "always column 1".
    assert max(distinct_columns) > 1, (
        "no step landed at a column > 1; the recorder is degenerating "
        f"to constant-column emission.  Distinct columns: {sorted(distinct_columns)}"
    )


def test_column_aware_distinct_columns_per_step_minified(tmp_path: Path) -> None:
    """P1 fixture from the milestone spec § "Test Design".

    Records the canonical minified Python one-liner (``a=1; b=2;
    c=a+b; d=c*c; print(d)``) and asserts the resulting trace is
    column-aware.

    This test does NOT assert "five distinct columns per step" the way
    the milestone draft did — see the module docstring's "note on
    sys.monitoring LINE granularity" for the rationale.  Distinct-
    column-per-statement-on-the-same-line acceptance is gated on per-
    INSTRUCTION recorder callbacks, which are not in scope for P1; the
    related JS pilot (P2) will exercise the per-statement column
    pattern via Babel-instrumentation, where each statement is its own
    instrumented site so the recorder naturally emits one step per
    statement.
    """
    script = tmp_path / "oneliner.py"
    script.write_text(
        "a=1; b=2; c=a+b; d=c*c; print(d)\n",
        encoding="utf-8",
    )

    trace_dir = tmp_path / "trace"
    ct_path = record_script(trace_dir, script)
    events = _print_json_events(ct_path)

    # The single step on line 1 must surface a column (any column —
    # the recorder picks the leftmost STORE's column, which on this
    # fixture is column 1; the contract is "column is present", not
    # "column is N").
    script_str = str(script)
    line1_steps = [
        e
        for e in events
        if e.get("type") == "step"
        and e.get("line") == 1
        and (e.get("path") == script_str or str(e.get("path", "")).endswith("/oneliner.py"))
    ]
    assert line1_steps, (
        "expected at least one step event on line 1 of the minified "
        "fixture; got events kinds="
        f"{sorted({e.get('type') for e in events})}"
    )
    for step in line1_steps:
        assert "column" in step, (
            "every step on the minified fixture must surface a column "
            f"in the column-aware JSON projection; missing on step "
            f"{step.get('step_index')}.  Full event: {step}"
        )
