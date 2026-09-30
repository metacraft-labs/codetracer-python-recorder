"""Correlation markers — end-to-end acceptance for the Python binding.

A *correlation marker* declares that a value crossed a boundary: put on a
queue here, taken off it there.  ``codetracer_python_recorder.markers`` is a
thin binding onto the shared CTFS writer library, which owns the payload and
the ``corrmark.ns`` index (contract:
``codetracer-specs/Testing/CTFS-Correlation-Marker-Contract.md`` §11a; user
spelling: ``GUI/Debugging-Features/Correlation-Markers.md`` §2.4).

**Nothing here is mocked, and that is the point.**  A marker's failure mode is
not a crash — it is a marker that is written but *unreadable*, which produces
an origin chain that stops early with nothing reporting an error.  A test
double asserting "we passed these strings to the writer" would pass against
exactly that bug.  So every assertion below runs a real recorded program
through the real recorder and reads the result back with **``ct print``** — the
canonical Nim decoder, the same one the debugger uses — via
``tests/python/support/ctfs.py``.

Three properties are checked, one per section:

* the marker reaches the container and decodes with ``correlation_marker`` /
  ``boundary_id`` / ``direction`` / ``key_value`` hoisted, ``show_text``
  preserved;
* a marker mints NO step (§11a.6) — it attaches to the enclosing one, so every
  step-addressed coordinate in the recording is unmoved;
* declaring a marker while nothing is recording is a silent ``False``, so a
  library may mark its boundaries unconditionally.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

import codetracer_python_recorder as codetracer

from .support.ctfs import ct_print_binary, ct_print_full, record_script

# The pairing domain used throughout.  Two markers pair only when they share
# it and carry opposing directions, so it is a wire value, not a label.
BOUNDARY = "order-processing"

# A syntactically valid OTel pair.  Deliberately mixed-case hex on the trace id:
# the shared library lowercases before decoding, and the index keys on the WIRE
# bytes — an index keyed on the hex rendering would be permanently unqueryable
# and silent about it.
TRACE_ID_HEX = "0AF7651916CD43DD8448EB211C80319C"
SPAN_ID_HEX = "b7ad6b7169203331"


def _write(tmp_path: Path, name: str, source: str) -> Path:
    """Materialise a recordable program and return its path."""
    script = tmp_path / name
    script.write_text(source)
    return script


def _markers_of(bundle: dict) -> list[dict]:
    """Every decoded correlation-marker payload in a ``ct print --full`` doc.

    ``ct print`` hoists a marker payload out of an IO event's metadata slot
    into ``correlation_marker`` (plus the three top-level fields consumers
    select on) precisely when the payload parses as one, so selecting on that
    key is also an assertion that the payload was *readable*.
    """
    return [event for event in bundle["events"] if "correlation_marker" in event]


def _steps_in(bundle: dict, path: Path) -> list[tuple[int, int]]:
    """The ``(line, column)`` sequence recorded for *path* itself.

    Projects to the recorded program's OWN file because the Python facade in
    ``markers.py`` is ordinary Python running in the recorded process, so its
    frames are recorded too — as they should be.  The claim a marker has to
    satisfy is about the *program's* timeline.
    """
    path_id = bundle["paths"].index(str(path))
    return [
        (int(event["line"]), int(event.get("column", 0)))
        for event in bundle["events"]
        if event.get("kind") == "step" and event.get("path_id") == path_id
    ]


# ---------------------------------------------------------------------------
# The marker reaches the container and decodes
# ---------------------------------------------------------------------------

# Each call is asserted INSIDE the recorded program.  Under an active
# recording every marker entry point returns True; the `False` no-op is
# reserved for "nothing is recording".  Asserting it here means a marker that
# silently no-opped while a session was live fails the recording itself,
# instead of surfacing later as an absent marker that could be blamed on the
# decoder.
MARKING_PROGRAM = f"""import codetracer_python_recorder as ct

assert ct.mark_correlation_send(
    {BOUNDARY!r}, key="order-42", show={{"id": 42}}, desc="Outbound order",
    show_text="body",
) is True
assert ct.mark_correlation_recv({BOUNDARY!r}, key="order-42") is True
assert ct.mark_span_coverage({TRACE_ID_HEX!r}, {SPAN_ID_HEX!r}, 111, 222) is True
"""


@pytest.fixture(scope="module")
def marked_bundle(tmp_path_factory) -> dict:
    """Record :data:`MARKING_PROGRAM` once and decode it with ``ct print``."""
    tmp_path = tmp_path_factory.mktemp("marked")
    script = _write(tmp_path, "marking_program.py", MARKING_PROGRAM)
    ct_path = record_script(tmp_path / "trace", script)
    return ct_print_full(ct_path)


def test_both_sides_of_a_boundary_crossing_land_in_the_container(marked_bundle):
    """Both declared crossings come back, in order, with their direction."""
    markers = _markers_of(marked_bundle)
    assert len(markers) == 2, (
        "expected exactly the two declared crossings; got "
        f"{json.dumps([m['correlation_marker'] for m in markers], indent=2)}"
    )
    assert [m["direction"] for m in markers] == ["send", "recv"], (
        "markers must decode in emission order carrying the direction they were "
        "declared with — the pairing index reads the two sides off this field"
    )


def test_the_selectors_a_consumer_keys_on_are_hoisted(marked_bundle):
    """``boundary_id`` / ``direction`` / ``key_value`` sit at the top level.

    A consumer selects markers on these three without descending into the
    payload, so they are part of the decode contract rather than a convenience.
    """
    send, recv = _markers_of(marked_bundle)
    for marker in (send, recv):
        assert marker["boundary_id"] == BOUNDARY
        assert marker["key_value"] == "order-42"
        # The hoisted fields must agree with the payload they were lifted from;
        # a divergence here would let a consumer select a marker and then read
        # different values off it.
        payload = marker["correlation_marker"]
        assert payload["boundary_id"] == marker["boundary_id"]
        assert payload["direction"] == marker["direction"]
        assert payload["key_value"] == marker["key_value"]


def test_show_and_description_survive_the_round_trip(marked_bundle):
    """``show_text`` is preserved verbatim when the caller passes one.

    Load-bearing rather than cosmetic: a cross-process origin chain resumes its
    walk on that NAME in the sending recording, so a marker that silently
    dropped it would be visible in the UI with its history unreachable — the
    exact "present but useless" failure this suite exists to catch.
    """
    send, recv = _markers_of(marked_bundle)
    send_payload = send["correlation_marker"]
    assert send_payload["show_text"] == "body"
    assert send_payload["show_value"] == "{'id': 42}", (
        "the binding stringifies the shown value in Python, before the writer "
        "lock is taken; the writer stores that text unaltered"
    )
    assert send_payload["description"] == "Outbound order"

    recv_payload = recv["correlation_marker"]
    assert "show_text" not in recv_payload, (
        "a crossing that declared no shown value must not invent one"
    )
    assert "description" not in recv_payload


def test_ct_print_markers_lists_them(tmp_path):
    """``ct print --markers`` — the operator-facing view — finds both.

    Guards the reader heuristic as well as the writer: this is the report that
    said ``correlation markers: 0`` for every CTFS recording before the shared
    marker API existed.
    """
    script = _write(tmp_path, "listing_program.py", MARKING_PROGRAM)
    ct_path = record_script(tmp_path / "trace", script)
    binary = ct_print_binary()
    assert binary.exists(), f"ct-print binary missing at {binary}"
    result = subprocess.run(
        [str(binary), "--markers", str(ct_path)],
        check=True,
        capture_output=True,
        text=True,
    )
    assert "correlation markers: 2" in result.stdout, result.stdout
    assert BOUNDARY in result.stdout


def test_span_coverage_is_accepted_and_carries_no_marker_payload(marked_bundle):
    """Span coverage is indexed, not turned into a pairable marker.

    A span-coverage declaration has no send/recv sense and no pairing domain,
    so forcing it into a ``MarkerPayload`` would make the pairing index try to
    pair spans with each other (contract §10.2).  It therefore contributes no
    IO event — the two crossings above are still the only two.

    That the call *reached an active writer* is asserted inside
    :data:`MARKING_PROGRAM`, which requires ``True``.  The resulting
    ``corrmark.ns`` index is not readable through ``ct print``; the writer
    library's own suite owns that assertion, and this test deliberately claims
    no more than it can observe.
    """
    assert len(_markers_of(marked_bundle)) == 2


def test_a_malformed_span_id_is_reported_not_swallowed(tmp_path):
    """Bad hex raises inside the recorded run rather than writing nothing.

    The whole campaign exists because an unqueryable index is indistinguishable
    from an absent one.  A recorder that accepted ``"zz"`` and quietly indexed
    nothing would reproduce that exact failure at the binding layer.
    """
    script = _write(
        tmp_path,
        "bad_hex.py",
        "import codetracer_python_recorder as ct\n"
        "ct.mark_span_coverage('zz', 'b7ad6b7169203331', 1, 2)\n",
    )
    with pytest.raises(codetracer.RecorderError) as excinfo:
        record_script(tmp_path / "trace", script)
    assert "32 hex characters" in str(excinfo.value)


# ---------------------------------------------------------------------------
# A marker mints no step (contract §11a.6)
# ---------------------------------------------------------------------------
#
# Minting one would insert an exec-stream event that no user code executed and
# shift every later step index — the indices spans' `start_step` / `end_step`,
# `ct_reader_step(n)` and the Request Panel's `startGeid` are all measured in.
#
# The control is `ensure_marker_id`: same module, same C-level shape, and it
# provably records nothing at all (it only interns a label).  So a difference
# between the two arms can only come from the crossing itself.

# Both arms are recorded through the NATIVE entry points rather than the Python
# facade, so the two programs differ by exactly one C call and nothing else.
# Going through `markers.py` would drag that module's own (correctly recorded)
# frames into the comparison and turn an exact equality into an approximate
# one; this arm is about the writer's decision, which is where the step would
# be minted.
_NATIVE_IMPORT = "from codetracer_python_recorder.codetracer_python_recorder import "

WITH_MARKERS = (
    _NATIVE_IMPORT + "mark_correlation\n"
    "\n"
    "def work(n):\n"
    f"    mark_correlation('send', {BOUNDARY!r}, str(n), None, None, None, None)\n"
    "    return n * 2\n"
    "\n"
    "total = 0\n"
    "for i in range(3):\n"
    "    total += work(i)\n"
)

WITH_SPAN_COVERAGE = (
    _NATIVE_IMPORT + "mark_span_coverage\n"
    "\n"
    "def work(n):\n"
    f"    mark_span_coverage({TRACE_ID_HEX!r}, {SPAN_ID_HEX!r}, 111, 222)\n"
    "    return n * 2\n"
    "\n"
    "total = 0\n"
    "for i in range(3):\n"
    "    total += work(i)\n"
)

WITHOUT_MARKERS = (
    _NATIVE_IMPORT + "ensure_marker_id\n"
    "\n"
    "def work(n):\n"
    f"    ensure_marker_id({BOUNDARY!r})\n"
    "    return n * 2\n"
    "\n"
    "total = 0\n"
    "for i in range(3):\n"
    "    total += work(i)\n"
)


def _record_and_decode(tmp_path: Path, name: str, source: str) -> tuple[Path, dict]:
    script = _write(tmp_path, name, source)
    ct_path = record_script(tmp_path / f"trace_{name}", script)
    return script, ct_print_full(ct_path)


def test_declaring_a_marker_does_not_change_the_step_count(tmp_path):
    """Same program, with and without three crossings — same steps."""
    marked_script, marked = _record_and_decode(tmp_path, "with.py", WITH_MARKERS)
    _, plain = _record_and_decode(tmp_path, "without.py", WITHOUT_MARKERS)

    assert len(_markers_of(marked)) == 3, "the marking arm must actually mark"
    assert _markers_of(plain) == [], "the control arm must declare no crossing"

    assert marked["counts"]["steps"] == plain["counts"]["steps"], (
        "a correlation marker minted a step: it must attach to the enclosing "
        "step, because inserting an exec-stream event shifts every later step "
        "index and every coordinate measured in them "
        f"(with markers: {marked['counts']['steps']}, "
        f"without: {plain['counts']['steps']})"
    )
    assert _steps_in(marked, marked_script) == _steps_in(plain, tmp_path / "without.py"), (
        "the recorded program's own (line, column) timeline must be unmoved"
    )


def test_declaring_span_coverage_does_not_change_the_step_count(tmp_path):
    """The same property for the span-coverage kind of marker.

    It takes a different path through the writer — no ``MarkerPayload``, no IO
    event — so "mints no step" has to be established for it separately rather
    than inherited from the boundary-crossing case.
    """
    covered_script, covered = _record_and_decode(tmp_path, "with_span.py", WITH_SPAN_COVERAGE)
    plain_script, plain = _record_and_decode(tmp_path, "no_span.py", WITHOUT_MARKERS)

    assert covered["counts"]["steps"] == plain["counts"]["steps"], (
        "declaring span coverage minted a step "
        f"(with: {covered['counts']['steps']}, without: {plain['counts']['steps']})"
    )
    assert _steps_in(covered, covered_script) == _steps_in(plain, plain_script)


def test_a_marker_attaches_to_a_step_that_exists(tmp_path):
    """Every marker's ``step_id`` names a step already in the exec stream.

    The complement of the count assertion: equal totals would also hold if a
    marker pointed at a step index past the end of the stream, which would be
    a coordinate no reader can resolve.
    """
    _, marked = _record_and_decode(tmp_path, "attaches.py", WITH_MARKERS)
    step_indices = {
        int(event["step_index"]) for event in marked["events"] if event.get("kind") == "step"
    }
    marker_steps = [int(m["step_id"]) for m in _markers_of(marked)]
    assert marker_steps, "no markers decoded — the arm is not testing anything"
    for step_id in marker_steps:
        assert step_id in step_indices, (
            f"marker attached to step {step_id}, which is not a recorded step; "
            "a marker must land on the enclosing step, not a synthesised one"
        )


def test_the_hoisted_marker_id_path_records_the_same_marker(tmp_path):
    """``ensure_marker_id`` + ``*_by_id`` is the primary, hot-path spelling.

    It must produce a marker indistinguishable from the string-label wrapper's
    — the wrapper interns and forwards to exactly this call, so a divergence
    would mean the two spellings write different things (§11a.4).
    """
    script = _write(
        tmp_path,
        "by_id.py",
        "import codetracer_python_recorder as ct\n"
        f"MARKER = ct.ensure_marker_id({BOUNDARY!r})\n"
        f"ct.mark_correlation_send_by_id(MARKER, {BOUNDARY!r}, key='order-42')\n"
        f"ct.mark_correlation_recv_by_id(MARKER, {BOUNDARY!r}, key='order-42')\n",
    )
    bundle = ct_print_full(record_script(tmp_path / "trace", script))
    markers = _markers_of(bundle)
    assert [m["direction"] for m in markers] == ["send", "recv"]
    for marker in markers:
        assert marker["boundary_id"] == BOUNDARY
        assert marker["key_value"] == "order-42"
        # The id the caller hoisted is the id the payload carries; the debugger
        # reads the label as text while the index keys on this integer.
        assert marker["correlation_marker"]["marker_id"] == 0, (
            "the first label interned in a container takes id 0"
        )


# ---------------------------------------------------------------------------
# No-op when not recording
# ---------------------------------------------------------------------------


def test_the_marker_api_is_a_silent_no_op_outside_a_recording():
    """Declaring a boundary in an unrecorded process returns falsey, silently.

    This is a contract, not an error path: a library marks its boundaries
    unconditionally and is imported into processes nobody records.  ``None``
    from :func:`ensure_marker_id` is deliberately distinct from marker id 0,
    which is a real id.
    """
    assert not codetracer.is_tracing(), "another test left a session running"

    assert codetracer.ensure_marker_id(BOUNDARY) is None
    assert codetracer.mark_correlation_send(BOUNDARY, key="order-42") is False
    assert codetracer.mark_correlation_recv(BOUNDARY, key="order-42") is False
    assert codetracer.mark_correlation_send_by_id(0, BOUNDARY, key="k") is False
    assert codetracer.mark_correlation_recv_by_id(0, BOUNDARY, key="k") is False
    assert codetracer.mark_span_coverage(TRACE_ID_HEX, SPAN_ID_HEX, 1, 2) is False

    # Not even the malformed input raises here: with nothing recording there is
    # no writer to reject it, and the no-op contract wins.
    assert codetracer.mark_span_coverage("zz", SPAN_ID_HEX, 1, 2) is False


def test_arbitrary_objects_are_stringified_by_the_binding(tmp_path):
    """``key`` / ``show`` accept any object; the binding renders them.

    The rendering happens in Python, in the caller's own frame, because
    ``__str__`` is user code and user code must never run while the recorder
    holds its writer lock — a raise from there could strand the guard.
    """

    script = _write(
        tmp_path,
        "objects.py",
        "import codetracer_python_recorder as ct\n"
        "\n"
        "class Order:\n"
        "    def __str__(self):\n"
        "        return 'order-99'\n"
        "\n"
        f"ct.mark_correlation_send({BOUNDARY!r}, key=Order(), show=[1, 2])\n",
    )
    bundle = ct_print_full(record_script(tmp_path / "trace", script))
    (marker,) = _markers_of(bundle)
    assert marker["key_value"] == "order-99"
    assert marker["correlation_marker"]["show_value"] == "[1, 2]"
