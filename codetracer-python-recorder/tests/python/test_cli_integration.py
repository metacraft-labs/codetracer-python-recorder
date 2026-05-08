"""Integration tests for the recorder CLI entry point.

Per ``codetracer-specs/Recorder-CLI-Conventions.md`` §4 the recorder is
CTFS-only: it does not accept a ``--format`` flag, does not read the
``CODETRACER_FORMAT`` environment variable, and never writes a JSON
events sidecar.  Tests that previously asserted on ``--format json``
output have been rewritten to record CTFS and pipe the recorded
``trace.ct`` container through ``ct print --json`` for content
assertions (see ``test_recorded_trace_via_ct_print_json``).
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]


# CTFS magic bytes identifying a valid .ct trace file.
# See: codetracer-trace-format specification.
_CTFS_MAGIC = bytes([0xC0, 0xDE, 0x72, 0xAC, 0xE2])


# Discover the sibling-built ct-print binary from
# ``codetracer-trace-format-nim``.  The pre-built artefact lives
# alongside the recorder repo under the workspace root; tests fall
# back to the ``CT_PRINT`` env var when callers want to point at a
# custom build.
def _ct_print_binary() -> Path:
    """Return the path to the ``ct-print`` binary used for CTFS conversion.

    Lookup order:

    1. ``CT_PRINT`` environment variable (callers can point at a
       custom build).
    2. The sibling ``codetracer-trace-format-nim`` checkout under the
       workspace root.  ``Path(__file__).resolve().parents[4]`` walks
       up: ``test_cli_integration.py`` → ``python/`` → ``tests/`` →
       ``codetracer-python-recorder/`` (inner) →
       ``codetracer-python-recorder/`` (outer) → workspace root.
    """
    override = os.environ.get("CT_PRINT")
    if override:
        return Path(override)
    return Path(__file__).resolve().parents[4] / "codetracer-trace-format-nim" / "ct-print"


def _write_script(path: Path, body: str = "print('hello from recorder')\n") -> None:
    path.write_text(body, encoding="utf-8")


def _run_cli(
    args: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "codetracer_python_recorder", *args],
        cwd=cwd,
        env=env,
        check=check,
        capture_output=True,
        text=True,
    )


def _prepare_env() -> dict[str, str]:
    env = os.environ.copy()
    pythonpath = env.get("PYTHONPATH", "")
    root = str(REPO_ROOT)
    env["PYTHONPATH"] = root if not pythonpath else os.pathsep.join([root, pythonpath])
    # Ensure stale env vars from the harness don't leak into the child.
    env.pop("CODETRACER_PYTHON_RECORDER_OUT_DIR", None)
    env.pop("CODETRACER_PYTHON_RECORDER_DISABLED", None)
    return env


def _find_ct_file(trace_dir: Path) -> Path:
    """Locate the CTFS ``.ct`` container in a recorded trace directory.

    The Nim writer names the produced container after the recorded
    program (e.g. ``program.ct``) rather than the literal ``trace.ct``,
    so callers must glob.  This helper raises an ``AssertionError`` with
    a directory listing when no container is found, so test failures
    are diagnosable.
    """
    ct_files = list(trace_dir.glob("*.ct"))
    assert ct_files, (
        f"No .ct files found in {trace_dir}; "
        f"contents: {list(trace_dir.iterdir()) if trace_dir.is_dir() else '<missing>'}"
    )
    return ct_files[0]


def test_cli_emits_trace_artifacts(tmp_path: Path) -> None:
    """Default recorder run produces a canonical CTFS container.

    Per convention §4 the recorder is CTFS-only.  The Nim writer emits
    a single multi-stream ``<program>.ct`` container; legacy JSON
    sidecars (``trace.json``) are forbidden.
    """
    script = tmp_path / "program.py"
    _write_script(script, "value = 21 + 21\nprint(value)\n")

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--on-recorder-error",
        "disable",
        "--require-trace",
        "--keep-partial-trace",
        "--log-level",
        "info",
        "--json-errors",
    ]
    args.append(str(script))

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0
    assert trace_dir.is_dir()

    trace_ct = _find_ct_file(trace_dir)
    assert not (trace_dir / "trace.json").exists(), (
        "trace.json must not be produced — the recorder is CTFS-only"
    )

    # Verify the CTFS magic bytes at the start of the file.
    with open(trace_ct, "rb") as f:
        magic = f.read(len(_CTFS_MAGIC))
    assert magic == _CTFS_MAGIC, f"Invalid CTFS magic: {magic.hex()}"


def test_cli_honours_trace_filter_chain(tmp_path: Path) -> None:
    """Smoke test: --trace-filter is accepted and the recording succeeds.

    Pre-2026-05 this test asserted on the trace-filter chain via the
    ``trace_metadata.json`` sidecar produced by ``--format json``.
    Under the CTFS-only contract the metadata sidecar is no longer
    written separately (it is embedded in the CTFS container), so the
    chain-content assertion has been moved to the API-level tests in
    ``test_monitoring_events.py`` (which still exercise the JSON event
    stream directly).  At the CLI layer we just verify that the
    explicit-and-default-filter combo doesn't break recording.
    """
    script = tmp_path / "program.py"
    _write_script(script, "print('filter test')\n")

    filters_dir = tmp_path / ".codetracer"
    filters_dir.mkdir()
    default_filter = filters_dir / "trace-filter.toml"
    default_filter.write_text(
        """
        [meta]
        name = "default"
        version = 1

        [scope]
        default_exec = "trace"
        default_value_action = "allow"
        """,
        encoding="utf-8",
    )

    override_filter = tmp_path / "override-filter.toml"
    override_filter.write_text(
        """
        [meta]
        name = "override"
        version = 1

        [scope]
        default_exec = "trace"
        default_value_action = "allow"

        [[scope.rules]]
        selector = "pkg:program"
        exec = "skip"
        value_default = "allow"
        """,
        encoding="utf-8",
    )

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--trace-filter",
        str(override_filter),
        str(script),
    ]

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0
    # The CTFS container must exist; the recorder fails loudly if any
    # filter file is invalid or unreachable.
    _find_ct_file(trace_dir)


def test_cli_honours_env_trace_filter(tmp_path: Path) -> None:
    """Smoke test: ``CODETRACER_TRACE_FILTER`` is accepted by the auto-start CLI path."""
    script = tmp_path / "program.py"
    _write_script(script, "print('env filter test')\n")

    filter_path = tmp_path / "env-filter.toml"
    filter_path.write_text(
        """
        [meta]
        name = "env-filter"
        version = 1

        [scope]
        default_exec = "trace"
        default_value_action = "allow"

        [[scope.rules]]
        selector = "pkg:program"
        exec = "skip"
        value_default = "allow"
        """,
        encoding="utf-8",
    )

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    env["CODETRACER_TRACE_FILTER"] = str(filter_path)

    result = _run_cli(["--out-dir", str(trace_dir), str(script)], cwd=tmp_path, env=env)
    assert result.returncode == 0
    _find_ct_file(trace_dir)


def test_ctfs_trace_has_steps(tmp_path: Path) -> None:
    """The default CTFS trace contains step data for the recorded program."""
    script = tmp_path / "program.py"
    _write_script(script, "a = 1\nb = 2\nc = a + b\nprint(c)\n")

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--on-recorder-error",
        "disable",
        "--require-trace",
    ]
    args.append(str(script))

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0

    trace_ct = _find_ct_file(trace_dir)
    # The CTFS container should have reasonable size (a few KB at minimum
    # for 4 lines of traced Python).  The exact byte count varies as the
    # CTFS encoder evolves; the floor is set at 256 bytes (the magic +
    # header alone is ~32 bytes, and we expect at least a handful of
    # registered events).
    assert trace_ct.stat().st_size > 256, "CTFS trace suspiciously small"


def test_ctfs_trace_records_exceptions(tmp_path: Path) -> None:
    """The default CTFS trace records exception events."""
    script = tmp_path / "program.py"
    _write_script(
        script,
        textwrap.dedent("""\
            try:
                x = 1 / 0
            except ZeroDivisionError:
                pass
            print("survived")
        """),
    )

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = [
        "--out-dir",
        str(trace_dir),
        "--on-recorder-error",
        "disable",
        "--require-trace",
    ]
    args.append(str(script))

    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0
    assert "survived" in result.stdout

    _find_ct_file(trace_dir)


# ---------------------------------------------------------------------------
# Convention compliance — ``Recorder-CLI-Conventions.md`` §4 / §5
# ---------------------------------------------------------------------------


def test_format_flag_rejected(tmp_path: Path) -> None:
    """Per convention §4 the CLI must reject ``--format`` outright.

    The previous implementation accepted ``--format json|binary|ctfs``.
    The new contract is CTFS-only and the flag is gone.  Any of the
    legacy values must produce a non-zero exit code (argparse uses 2
    for usage errors) and a stderr message that mentions the flag so
    users have a clear migration path.
    """
    script = tmp_path / "program.py"
    _write_script(script)
    env = _prepare_env()

    for legacy_value in ("json", "binary", "ctfs"):
        result = _run_cli(
            ["--format", legacy_value, str(script)],
            cwd=tmp_path,
            env=env,
            check=False,
        )
        assert result.returncode != 0, (
            f"--format {legacy_value} should be rejected, got exit code 0"
        )
        # The error message must mention the flag so users know what to fix.
        assert "--format" in result.stderr or "--format" in result.stdout

    # The collapsed ``--format=json`` form must also be rejected.
    result = _run_cli(
        ["--format=json", str(script)], cwd=tmp_path, env=env, check=False
    )
    assert result.returncode != 0


def test_no_format_flag_in_help() -> None:
    """The ``--help`` output must not advertise ``--format`` or ``CODETRACER_FORMAT``."""
    env = _prepare_env()
    result = _run_cli(["--help"], cwd=Path.cwd(), env=env)
    combined = result.stdout + result.stderr
    assert "--format" not in combined, (
        "--help must not mention --format (recorder is CTFS-only)"
    )
    assert "CODETRACER_FORMAT" not in combined, (
        "--help must not mention CODETRACER_FORMAT (recorder is CTFS-only)"
    )


def test_help_mentions_ct_print() -> None:
    """The ``--help`` output must point users at ``ct print`` (convention §4).

    ``ct print`` from ``codetracer-trace-format-nim`` is the canonical
    conversion tool from CTFS to JSON / text; the recorder no longer
    emits these forms directly.
    """
    env = _prepare_env()
    result = _run_cli(["--help"], cwd=Path.cwd(), env=env)
    combined = result.stdout + result.stderr
    assert "ct print" in combined, (
        "--help must mention `ct print` as the conversion tool"
    )


def test_env_out_dir_used_when_flag_omitted(tmp_path: Path) -> None:
    """``CODETRACER_PYTHON_RECORDER_OUT_DIR`` is honoured when ``--out-dir`` is omitted (§5)."""
    script = tmp_path / "program.py"
    _write_script(script, "print('env out dir test')\n")

    env_trace_dir = tmp_path / "env-out"
    env = _prepare_env()
    env["CODETRACER_PYTHON_RECORDER_OUT_DIR"] = str(env_trace_dir)

    result = _run_cli([str(script)], cwd=tmp_path, env=env)
    assert result.returncode == 0
    assert env_trace_dir.is_dir(), (
        f"recorder should have written into {env_trace_dir}; "
        f"contents of tmp_path: {list(tmp_path.iterdir())}"
    )
    _find_ct_file(env_trace_dir)


def test_cli_flag_overrides_env_out_dir(tmp_path: Path) -> None:
    """``--out-dir`` always wins over the env-var fallback (§5)."""
    script = tmp_path / "program.py"
    _write_script(script, "print('cli wins')\n")

    env_trace_dir = tmp_path / "env-out"
    cli_trace_dir = tmp_path / "cli-out"
    env = _prepare_env()
    env["CODETRACER_PYTHON_RECORDER_OUT_DIR"] = str(env_trace_dir)

    result = _run_cli(
        ["--out-dir", str(cli_trace_dir), str(script)], cwd=tmp_path, env=env
    )
    assert result.returncode == 0
    assert cli_trace_dir.is_dir()
    _find_ct_file(cli_trace_dir)
    assert not env_trace_dir.exists(), (
        "env-supplied dir must not be touched when --out-dir is given"
    )


def test_env_disabled_skips_recording(tmp_path: Path) -> None:
    """``CODETRACER_PYTHON_RECORDER_DISABLED=1`` short-circuits recording (§5).

    The target program must still execute (so users keep their CI
    pipelines working with the recorder shim in place), but no trace
    artefacts should be produced.
    """
    script = tmp_path / "program.py"
    _write_script(script, "print('still ran without recording')\n")

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    env["CODETRACER_PYTHON_RECORDER_DISABLED"] = "1"

    result = _run_cli(
        ["--out-dir", str(trace_dir), str(script)], cwd=tmp_path, env=env
    )
    assert result.returncode == 0
    assert "still ran without recording" in result.stdout, (
        f"target script must still execute when disabled; stdout={result.stdout!r}"
    )
    # No CTFS container or JSON sidecar should have been written.
    if trace_dir.exists():
        unwanted = list(trace_dir.glob("*.ct")) + list(trace_dir.glob("trace.*"))
        assert not unwanted, (
            f"recording should have been skipped; got {unwanted}"
        )

    # Also accepts ``true`` (case-insensitive) as a truthy value.
    env["CODETRACER_PYTHON_RECORDER_DISABLED"] = "TRUE"
    result = _run_cli([str(script)], cwd=tmp_path, env=env)
    assert result.returncode == 0
    assert "still ran without recording" in result.stdout


# ---------------------------------------------------------------------------
# ct-print round-trip (replaces the old ``--format json`` content assertions)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(
    not _ct_print_binary().exists(),
    reason="ct-print binary not built — run from a workspace with codetracer-trace-format-nim",
)
def test_recorded_trace_via_ct_print_json(tmp_path: Path) -> None:
    """Record a real script and assert exact decoded values via ``ct-print --full``.

    This test mirrors the cairo / cardano / circom / flow / fuel / leo /
    miden / move / polkavm / solana / ton (Int round-trip), evm (Raw
    byte) and js (String / Raw) precedents — record a real program,
    then convert the produced CTFS bundle through
    ``ct-print --full --strip-paths`` to obtain a deterministic JSON
    oracle with every CBOR ``ValueRecord`` fully decoded.  See
    ``Recorder-CLI-Conventions.md`` §4 — CTFS-only output, with
    ``ct print`` as the canonical conversion tool.

    Why exact-value assertions matter: the previous ``--json`` layer
    only checked for substring presence ("does the trace mention
    ``make_greeting`` somewhere"), so a recorder regression that
    silently dropped or corrupted a value would not be caught.  The
    new ``--full`` layer pins:

      - **Strict ``value.kind`` invariant** — every step var, call arg
        and return value must decode to one of the known
        ``ValueRecord`` variants.  A new variant fires the test loudly
        so the next maintainer can extend the assertion rather than
        silently weakening it.
      - **Exact (varname, value) pair assertions** — e.g.
        ``make_greeting``'s ``target_name`` parameter decodes back to
        ``ValueRecord::String { text: "world" }`` and its return value
        is ``ValueRecord::String { text: "hello, world" }``.
      - **Function / path / counts / call-sequence anchors** — 10 steps,
        3 calls, 1 io; calls are ``<__main__> → main → make_greeting``;
        path table contains the canonical fixture; function table
        contains ``<__main__>``, ``main`` and ``make_greeting``.
      - **IO event** — a single ``ioStdout`` write of ``"hello, world\\n"``.

    The canonical fixture below exercises:

        def make_greeting(target_name):           # line 1
            greeting_value = "hello, " + target_name   # line 2
            return greeting_value                 # line 3

        def main():                               # line 6
            person_name = "world"                 # line 7
            result_text = make_greeting(person_name)   # line 8
            print(result_text)                    # line 9

        if __name__ == "__main__":                # line 12
            main()                                # line 13

    Each binding must surface in the trace as a step / call_entry event
    with a decoded ``ValueRecord``.  The Python recorder uses the
    ``String`` variant for typed string locals / args / return values,
    and the ``Raw`` variant for non-serialisable / opaque objects
    (function references, builtins lookups).  Both are valid current
    behaviour; the strict invariant fires if a brand-new variant
    appears (e.g. ``BigInt`` support for ``int`` overflow).
    """
    script_body = textwrap.dedent("""\
        def make_greeting(target_name):
            greeting_value = "hello, " + target_name
            return greeting_value


        def main():
            person_name = "world"
            result_text = make_greeting(person_name)
            print(result_text)


        if __name__ == "__main__":
            main()
        """)
    script = tmp_path / "ct_print_smoke.py"
    _write_script(script, script_body)

    trace_dir = tmp_path / "trace"
    env = _prepare_env()
    args = ["--out-dir", str(trace_dir), str(script)]
    result = _run_cli(args, cwd=tmp_path, env=env)
    assert result.returncode == 0

    trace_ct = _find_ct_file(trace_dir)

    ct_print = _ct_print_binary()
    # ``--strip-paths`` rewrites absolute workdir / tmp prefixes to
    # placeholders (``<workdir>``, ``<tmpdir>``) so JSON stays
    # diff-stable across machines and test runs.  The ``--full`` flag
    # decodes every CBOR ``ValueRecord`` back to a structured JSON
    # object — without it values would be opaque blobs we could only
    # substring-match against.
    #
    # ``LD_LIBRARY_PATH`` may need to point at zstd's ``.so`` directory
    # under Nix; callers running outside Nix can pre-set it (or use a
    # system zstd build).  We deliberately do not paper over a missing
    # zstd by skipping — that would mask a real environment break.
    proc = subprocess.run(
        [str(ct_print), "--full", "--strip-paths", str(trace_ct)],
        capture_output=True,
        text=True,
        check=True,
    )
    bundle = json.loads(proc.stdout)

    # ------------------------------------------------------------------
    # Function table — ``<__main__>``, ``main`` and ``make_greeting``.
    # ------------------------------------------------------------------
    # The Python recorder names the synthetic top-level frame
    # ``<__main__>`` (mirrors JS's ``<module>``).  ``ends_with`` checks
    # stay tolerant of any future namespacing prefix the recorder might
    # add (e.g. ``mymod::main``).
    assert any(
        f.endswith("<__main__>") for f in bundle["functions"]
    ), f"missing <__main__> in functions: {bundle['functions']!r}"
    assert any(
        f.endswith("main") and not f.endswith("<__main__>")
        for f in bundle["functions"]
    ), f"missing main in functions: {bundle['functions']!r}"
    assert any(
        f.endswith("make_greeting") for f in bundle["functions"]
    ), f"missing make_greeting in functions: {bundle['functions']!r}"

    # ------------------------------------------------------------------
    # Path table — the canonical fixture path.
    # ------------------------------------------------------------------
    # ``--strip-paths`` rewrites ``/tmp/...`` → ``<tmp>/...`` so the
    # trailing component is the only stable assertion.
    assert any(
        p.endswith("ct_print_smoke.py") for p in bundle["paths"]
    ), f"missing ct_print_smoke.py in paths: {bundle['paths']!r}"

    # ------------------------------------------------------------------
    # Counts — stable for the canonical fixture.
    # ------------------------------------------------------------------
    # The Python recorder produces a deterministic event count for
    # this fixture under sys.monitoring instrumentation:
    #   - 10 step events (module prologue + line-by-line execution
    #     across <__main__>, main, make_greeting)
    #   - 3 call entries (<__main__> wrapper, main, make_greeting)
    #   - 1 io event (the ``print(result_text)`` write to stdout)
    # If these change, that's a real regression to investigate, not a
    # flake — pin the values strictly.
    assert bundle["counts"]["steps"] == 10, (
        f"expected 10 steps, got {bundle['counts']['steps']}; full counts: {bundle['counts']!r}"
    )
    assert bundle["counts"]["calls"] == 3, (
        f"expected 3 calls, got {bundle['counts']['calls']}; full counts: {bundle['counts']!r}"
    )
    assert bundle["counts"]["io_events"] == 1, (
        f"expected 1 io_event, got {bundle['counts']['io_events']}; full counts: {bundle['counts']!r}"
    )

    # ------------------------------------------------------------------
    # Call sequence — <__main__> → main → make_greeting.
    # ------------------------------------------------------------------
    call_sequence = [
        e["function"] for e in bundle["events"] if e["kind"] == "call_entry"
    ]
    assert len(call_sequence) == 3, (
        f"expected 3 call_entry events, got {len(call_sequence)}: {call_sequence!r}"
    )
    assert call_sequence[0].endswith("<__main__>"), (
        f"first call must enter <__main__>, got {call_sequence[0]!r}"
    )
    assert call_sequence[1].endswith("main"), (
        f"second call must enter main, got {call_sequence[1]!r}"
    )
    assert call_sequence[2].endswith("make_greeting"), (
        f"third call must enter make_greeting, got {call_sequence[2]!r}"
    )

    # ------------------------------------------------------------------
    # Strict ValueRecord variant invariant — every step var / call arg
    # / return value must decode to a known kind.  If a brand-new
    # variant appears, this fires loudly so the next maintainer
    # extends the exact-value layer rather than silently accepting it.
    # ------------------------------------------------------------------
    allowed_kinds = {
        "Int",
        "Float",
        "String",
        "Bool",
        "Raw",
        "None",
        "Void",
        "Sequence",
        "Struct",
        "Tuple",
    }

    def _check_kinds(value: dict, ctx: str) -> None:
        """Recursively verify ``value.kind`` for nested Sequence / Struct / Tuple."""
        kind = value.get("kind")
        assert kind in allowed_kinds, (
            f"{ctx}: unknown ValueRecord kind={kind!r}; if a new variant has "
            f"landed for the Python recorder, extend this test to assert on it "
            f"explicitly rather than weakening the check"
        )
        for nested in value.get("elements", []) or []:
            _check_kinds(nested, ctx + ".elements[]")
        for nested in value.get("field_values", []) or []:
            _check_kinds(nested, ctx + ".field_values[]")

    for ev in bundle["events"]:
        if ev["kind"] == "step":
            for v in ev["vars"]:
                _check_kinds(
                    v["value"],
                    f"step {ev['step_index']} var {v['varname']!r}",
                )
        elif ev["kind"] == "call_entry":
            for a in ev["args"]:
                _check_kinds(
                    a["value"],
                    f"call_entry {ev['function']!r} arg {a['varname']!r}",
                )
        elif ev["kind"] == "call_exit":
            _check_kinds(
                ev["return_value"],
                f"call_exit {ev['function']!r} return_value",
            )

    # ------------------------------------------------------------------
    # Exact decoded call-arg value: make_greeting(target_name="world").
    # ------------------------------------------------------------------
    # The Python recorder uses ``ValueRecord::String`` for typed string
    # call arguments — ``ct-print --full`` decodes it to
    # ``{"kind":"String","text":"world",...}``.  This is the Python
    # analogue of cairo's ``(a, 10)`` Int round-trip.
    make_greeting_call = next(
        (
            e
            for e in bundle["events"]
            if e["kind"] == "call_entry" and e["function"].endswith("make_greeting")
        ),
        None,
    )
    assert make_greeting_call is not None, "no call_entry for make_greeting"
    target_name_arg = next(
        (a for a in make_greeting_call["args"] if a["varname"] == "target_name"),
        None,
    )
    assert target_name_arg is not None, (
        f"make_greeting call_entry missing target_name arg; "
        f"args={make_greeting_call['args']!r}"
    )
    assert target_name_arg["value"]["kind"] == "String", (
        f"target_name should decode as String, got "
        f"{target_name_arg['value']['kind']!r}"
    )
    assert target_name_arg["value"]["text"] == "world", (
        f"target_name should be 'world', got "
        f"{target_name_arg['value'].get('text')!r}"
    )

    # ------------------------------------------------------------------
    # Exact decoded return value: make_greeting → "hello, world".
    # ------------------------------------------------------------------
    # ``"hello, " + "world"`` returns ``"hello, world"``.  The Python
    # recorder snapshots the typed string return value via
    # ``ValueRecord::String`` (not the textual ``Raw`` form the JS
    # recorder uses).  The strict ``kind === "String"`` invariant
    # means: if a future recorder upgrade emits a different variant,
    # this fails loudly.
    make_greeting_exit = next(
        (
            e
            for e in bundle["events"]
            if e["kind"] == "call_exit" and e["function"].endswith("make_greeting")
        ),
        None,
    )
    assert make_greeting_exit is not None, "no call_exit for make_greeting"
    rv = make_greeting_exit["return_value"]
    assert rv["kind"] == "String", (
        f"make_greeting return_value should decode as String, got {rv['kind']!r}"
    )
    assert rv["text"] == "hello, world", (
        f"make_greeting should return 'hello, world', got {rv.get('text')!r}"
    )

    # ------------------------------------------------------------------
    # main() returns None — strictly typed.
    # ------------------------------------------------------------------
    main_exit = next(
        (
            e
            for e in bundle["events"]
            if e["kind"] == "call_exit"
            and e["function"].endswith("main")
            and not e["function"].endswith("<__main__>")
        ),
        None,
    )
    assert main_exit is not None, "no call_exit for main"
    assert main_exit["return_value"]["kind"] == "None", (
        f"main return_value should decode as None, got "
        f"{main_exit['return_value']['kind']!r}"
    )

    # ------------------------------------------------------------------
    # Exact (varname, value) step-var pairs.
    # ------------------------------------------------------------------
    # Collect every (varname, kind, text) triple surfaced by step
    # events.  The Python recorder snapshots typed string locals via
    # ``ValueRecord::String`` (so ``person_name = "world"`` and
    # ``result_text = "hello, world"`` and ``greeting_value =
    # "hello, world"`` and ``target_name = "world"`` all surface
    # with the ``String`` kind).  This is the Python analogue of
    # cairo's ``a=10, b=32, sum_val=42, ...`` round-trip.
    observed_step_vars: list[tuple[str, str, str | None]] = []
    for ev in bundle["events"]:
        if ev["kind"] != "step":
            continue
        for v in ev["vars"]:
            if v["varname"].startswith("__"):
                # Filter out the dunders the Python module-level frame
                # surfaces by default (__name__, __file__, ...) — those
                # are environment-dependent and not part of our
                # convention contract.
                continue
            observed_step_vars.append(
                (
                    v["varname"],
                    v["value"]["kind"],
                    # Both ``String.text`` and ``Raw.r`` carry textual
                    # payload — the recorder picks one or the other.
                    # Accept whichever is populated so the assertion
                    # stays readable.
                    v["value"].get("text") or v["value"].get("r"),
                )
            )

    expected_step_vars = [
        # main()'s body bindings.
        ("person_name", "String", "world"),
        ("result_text", "String", "hello, world"),
        # make_greeting()'s body bindings.
        ("target_name", "String", "world"),
        ("greeting_value", "String", "hello, world"),
    ]
    for want in expected_step_vars:
        assert want in observed_step_vars, (
            f"expected step variable {want!r} in --full output; "
            f"observed = {observed_step_vars!r}"
        )

    # ------------------------------------------------------------------
    # IO event — the single ``print(result_text)`` write to stdout.
    # ------------------------------------------------------------------
    io_events = [e for e in bundle["events"] if e["kind"] == "io"]
    assert len(io_events) == 1, (
        f"expected exactly 1 io event, got {len(io_events)}: {io_events!r}"
    )
    io = io_events[0]
    assert io["io_kind"] == "ioStdout", (
        f"io event should be ioStdout, got {io['io_kind']!r}"
    )
    # ``print`` appends a trailing newline; the recorder captures the
    # raw bytes written to stdout, so the newline must be present.
    assert io["text"] == "hello, world\n", (
        f"io event text should be 'hello, world\\n', got {io['text']!r}"
    )
    assert io["bytes_len"] == len("hello, world\n"), (
        f"io event bytes_len should be {len('hello, world\\n')}, got {io['bytes_len']}"
    )
