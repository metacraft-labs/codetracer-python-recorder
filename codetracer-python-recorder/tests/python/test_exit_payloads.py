"""Exit-payload coverage for the recorder.

The recorder records the process / session exit status as the
*top-level* return value of the recorded trace: ``sys.exit(N)`` unwinds
the synthetic ``<__main__>`` frame carrying the ``SystemExit`` object,
and a session stopped without a script exit code records the ``<exit>``
placeholder.  These tests pin that behaviour.

Per ``codetracer-specs/Recorder-CLI-Conventions.md`` §4 the recorder is
CTFS-only: it emits a single ``<program>.ct`` container and never the
legacy ``trace.json`` / ``trace_metadata.json`` sidecars (retired in
commit ``efcfe28``, "#254 Phase 2").  The pre-CTFS version of this file
read the exit value from the ``trace.json`` event stream and the
``process_exit_status`` field of the ``trace_metadata.json`` sidecar.
Both sidecars are gone; the exit value is now decoded from the ``.ct``
container's call stream via ``ct-print --full`` — the canonical CTFS
reader from ``codetracer-trace-format-nim``, exactly as
``test_cli_integration.py`` does.  The asserted fact — that the
recorder records the exact exit code / placeholder as the top-level
return — is unchanged in strength: a recorder that drops or corrupts
the exit payload still fails the test loudly.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

from .support.ctfs import ct_print_full, find_ct_file


REPO_ROOT = Path(__file__).resolve().parents[2]


def _all_return_values(trace_dir: Path) -> list[dict[str, object]]:
    """Return every decoded ``call_exit.return_value`` in event order.

    ``ct-print --full`` materialises each recorded frame return as a
    ``call_exit`` event carrying the decoded ValueRecord.  Raises an
    ``AssertionError`` (rather than returning an empty list) when the
    trace has no returns at all, so a recorder that records nothing
    fails loudly.
    """
    bundle = ct_print_full(find_ct_file(trace_dir))
    returns = [
        event["return_value"]
        for event in bundle["events"]
        if event.get("kind") == "call_exit"
    ]
    assert returns, (
        "trace did not contain any call_exit return values; "
        f"event kinds: {[e.get('kind') for e in bundle['events']]}"
    )
    return returns


def _toplevel_exit_code_text(trace_dir: Path) -> str:
    """Return the exit code recorded by a CLI ``sys.exit(N)`` run.

    A CLI script run has a single synthetic depth-0 ``<__main__>``
    frame.  ``sys.exit(N)`` unwinds it carrying the ``SystemExit``
    object, which ``ct-print --full`` decodes as the frame's
    ``return_value`` — a ``Raw`` ValueRecord whose ``r`` payload is
    ``str(N)``.  Raises an ``AssertionError`` when no such frame is
    present so a dropped/renamed top-level frame fails loudly.
    """
    bundle = ct_print_full(find_ct_file(trace_dir))
    toplevel_exits = [
        event
        for event in bundle["events"]
        if event.get("kind") == "call_exit" and event.get("depth") == 0
    ]
    assert toplevel_exits, (
        "trace did not contain a depth-0 call_exit for the <__main__> frame; "
        f"events: {[e.get('kind') for e in bundle['events']]}"
    )
    assert len(toplevel_exits) == 1, (
        "expected exactly one depth-0 frame for a CLI script run, got "
        f"{[e.get('function') for e in toplevel_exits]}"
    )
    value = toplevel_exits[0]["return_value"]
    kind = value.get("kind")
    # ``sys.exit(N)`` unwinds the frame with the SystemExit object,
    # decoded as Raw; an integer-typed return would also be acceptable.
    if kind == "Raw":
        return str(value.get("r"))
    if kind == "Int":
        return str(value.get("i"))
    raise AssertionError(
        f"top-level exit return must encode the exit code, got {value!r}"
    )


def test_cli_records_exit_code_in_toplevel_return(tmp_path: Path) -> None:
    script = tmp_path / "exit_script.py"
    script.write_text(
        "import sys\n"
        "sys.exit(3)\n",
        encoding="utf-8",
    )

    trace_dir = tmp_path / "trace"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "codetracer_python_recorder",
            "--out-dir",
            str(trace_dir),
            str(script),
        ],
        capture_output=True,
        text=True,
        check=False,
    )

    # Default policy: the recorder does not propagate the script exit
    # code, so the process itself exits 0 but logs the observed status.
    assert result.returncode == 0, result.stderr
    assert "status 3; returning 0" in result.stderr

    # The recorder must record exit code 3 as the top-level return value:
    # ``sys.exit(3)`` unwinds ``<__main__>`` carrying ``SystemExit(3)``.
    assert _toplevel_exit_code_text(trace_dir) == "3", (
        "expected exit code 3 in the top-level return"
    )


def test_cli_can_propagate_script_exit(tmp_path: Path) -> None:
    script = tmp_path / "exit_script.py"
    script.write_text(
        "import sys\n"
        "sys.exit(5)\n",
        encoding="utf-8",
    )

    trace_dir = tmp_path / "trace"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "codetracer_python_recorder",
            "--out-dir",
            str(trace_dir),
            "--propagate-script-exit",
            str(script),
        ],
        capture_output=True,
        text=True,
        check=False,
    )

    # With --propagate-script-exit the recorder mirrors the script's exit
    # code and does NOT print the "returning 0" downgrade notice.
    assert result.returncode == 5, result.stderr
    assert "status 5; returning 0" not in result.stderr

    # The recorder must still record exit code 5 as the top-level return.
    assert _toplevel_exit_code_text(trace_dir) == "5", (
        "expected exit code 5 in the top-level return"
    )


def test_default_exit_payload_uses_placeholder(tmp_path: Path) -> None:
    trace_dir = tmp_path / "trace"
    trace_dir.mkdir()

    # Directly call the start/stop API without providing an exit code.
    # A session stopped this way records the ``<exit>`` placeholder as the
    # top-level return value rather than a concrete integer.
    script = (
        "import json\n"
        "from pathlib import Path\n"
        "import codetracer_python_recorder as codetracer\n"
        f"trace_dir = Path({json.dumps(str(trace_dir))!s})\n"
        "session = codetracer.start(trace_dir)\n"
        "session.stop()\n"
    )
    subprocess.run([sys.executable, "-c", script], check=True)

    # ``emit_session_exit`` registers exactly one session-exit return; for
    # a session stopped without an exit code it is the ``<exit>`` String
    # placeholder.  It must be present and must be that exact sentinel.
    returns = _all_return_values(trace_dir)
    placeholders = [
        value
        for value in returns
        if value.get("kind") == "String" and value.get("text") == "<exit>"
    ]
    assert len(placeholders) == 1, (
        "expected exactly one '<exit>' String placeholder among the recorded "
        f"return values, got {returns!r}"
    )
