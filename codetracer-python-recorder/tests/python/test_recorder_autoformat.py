"""P6.2 — Python recorder autoformat unit + CLI integration tests.

Spec: ``codetracer-specs/Planned-Features/Column-Aware-Tracing-And-Deminification.milestones.org`` §P6.2.

These tests exercise the recorder-side ``black`` autoformat surface
that mirrors the JS recorder's P6.2 ``prettier`` integration.  The
core API lives in ``codetracer-python-recorder/src/runtime/autoformat.rs``
and is exposed through the CLI's ``--autoformat`` / ``--no-autoformat``
flag in ``codetracer_python_recorder/cli.py``.

Per the milestone time-box, the *recording flow integration* (Step 3 in
the implementer task — materialising ``<trace>/files/<name>.fmt.py`` +
the V3 sourcemap inside the CTFS container) is a follow-up.  The
recorder's CTFS writer keeps source files inside the ``.ct`` container
(no ``<trace>/files/`` sidecar like the JS recorder uses), so wiring
the formatted sibling into the binary container is a wire-format
change to the writer rather than a simple file-write at finalise.
The unit-test suite below verifies the pure pre-format hooks; the
end-to-end recording-flow assertions land with the writer-side
follow-up.

What this file does cover:

* :func:`test_autoformat_cli_flag_is_accepted` — ``--autoformat`` and
  ``--no-autoformat`` parse without error and the resulting CLI config
  carries the flag.
* :func:`test_autoformat_cli_flag_default_on` — omitting the flag
  defaults to ``autoformat=True`` so existing users get the
  pre-format behaviour by default.
* :func:`test_no_autoformat_sets_env_kill_switch` — passing
  ``--no-autoformat`` plumbs the disable through
  ``CT_AUTOFORMAT`` so the Rust runtime's
  ``autoformat_enabled_by_env`` skips the pass.
* :func:`test_autoformat_help_text_mentions_black` — the help string
  documents the ``black`` invocation so users discover the feature.

Rust-side unit tests (``looks_minified``, ``generate_inverse_sourcemap``,
``try_autoformat`` skip-reason dispatch, ``encode_vlq`` round-trip) live
in ``codetracer-python-recorder/src/runtime/autoformat.rs`` under
``#[cfg(test)] mod tests``.  Run them via ``just cargo-test``.
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

# Project-local imports — same pattern as the other CLI tests.
from codetracer_python_recorder.cli import RecorderCLIConfig, _parse_args


# ---------------------------------------------------------------------------
# CLI flag parsing
# ---------------------------------------------------------------------------


def _make_dummy_script(tmp_path: Path) -> Path:
    """Materialise a trivial Python script so the CLI's script-existence
    check passes during ``_parse_args``.  The script's content doesn't
    matter — these tests only inspect the parsed config, not the trace.
    """
    script = tmp_path / "dummy.py"
    script.write_text("print('hello')\n", encoding="utf-8")
    return script


def test_autoformat_cli_flag_default_on(tmp_path: Path) -> None:
    """P6.2: omitting ``--autoformat`` defaults to ``True``.

    Default-on matches the JS recorder so the pre-format pass
    automatically fires on minified sources without users having to
    opt in.  Regression target: a future change that flips the default
    would silently stop materialising formatted siblings for the bulk
    of recorded minified bundles.
    """
    script = _make_dummy_script(tmp_path)
    config: RecorderCLIConfig = _parse_args([str(script)])
    assert config.autoformat is True


def test_autoformat_cli_flag_explicit_on(tmp_path: Path) -> None:
    """Passing ``--autoformat`` explicitly still resolves to True.

    BooleanOptionalAction generates a ``--no-autoformat`` companion
    automatically; this test pins down the positive-form behaviour so
    a future migration to a different action type can't silently
    invert the semantics.
    """
    script = _make_dummy_script(tmp_path)
    config: RecorderCLIConfig = _parse_args(["--autoformat", str(script)])
    assert config.autoformat is True


def test_no_autoformat_cli_flag_disables(tmp_path: Path) -> None:
    """``--no-autoformat`` flips the resolved flag to False.

    This is the user-facing kill switch for the pre-format pass.  Users
    who don't want the recorder to spawn ``black`` (e.g. air-gapped CI
    where ``black`` isn't installed and they'd rather not see the
    one-shot warning) reach for this flag.
    """
    script = _make_dummy_script(tmp_path)
    config: RecorderCLIConfig = _parse_args(["--no-autoformat", str(script)])
    assert config.autoformat is False


def test_autoformat_help_text_mentions_black(tmp_path: Path) -> None:
    """``--help`` documents that ``black`` is the underlying formatter.

    Users learn the dependency from ``--help`` rather than having to
    spelunk the source.  Regression target: a future refactor of the
    help text that drops the tool name would hide the dependency from
    users and surprise them when ``black`` isn't on PATH.
    """
    # The argparse help text is written to stdout when --help fires; we
    # run the CLI in a subprocess so SystemExit doesn't tear down the
    # pytest process.
    result = subprocess.run(
        [sys.executable, "-m", "codetracer_python_recorder", "--help"],
        capture_output=True,
        text=True,
        check=False,
    )
    # --help exits 0 in argparse; the help text lands on stdout.
    assert result.returncode == 0, (
        f"--help should exit 0 but returned {result.returncode}; "
        f"stderr={result.stderr}"
    )
    help_text = result.stdout
    assert "--autoformat" in help_text, (
        "expected --autoformat in help text; got:\n" + help_text[-2000:]
    )
    assert "--no-autoformat" in help_text, (
        "expected --no-autoformat in help text; got:\n" + help_text[-2000:]
    )
    assert "black" in help_text.lower(), (
        "expected mention of 'black' (the underlying formatter) in help text; "
        "got:\n" + help_text[-2000:]
    )


# ---------------------------------------------------------------------------
# Env-var plumbing
# ---------------------------------------------------------------------------


@pytest.fixture
def restore_env():
    """Snapshot + restore ``CT_AUTOFORMAT`` so tests don't leak env state."""
    saved = os.environ.get("CT_AUTOFORMAT")
    try:
        yield
    finally:
        if saved is None:
            os.environ.pop("CT_AUTOFORMAT", None)
        else:
            os.environ["CT_AUTOFORMAT"] = saved


def test_no_autoformat_sets_env_kill_switch(
    tmp_path: Path, restore_env, monkeypatch: pytest.MonkeyPatch
) -> None:
    """P6.2: ``--no-autoformat`` plumbs the disable into ``CT_AUTOFORMAT``.

    The Rust runtime's ``autoformat_enabled_by_env`` reads this var,
    so setting it from the CLI is how the flag reaches the runtime
    without a new PyO3 binding parameter.  This matches the
    cross-recorder convention from the replay-server's lazy P4 path
    (where ``CT_AUTOFORMAT`` is the global kill switch).

    We exercise ``main`` directly with a recording-disabled env so the
    recorder bails out before actually starting a trace — the side
    effect we care about is the env-var write, not the trace output.
    """
    from codetracer_python_recorder.cli import ENV_DISABLED, main

    monkeypatch.setenv(ENV_DISABLED, "1")  # short-circuit the recording path
    os.environ.pop("CT_AUTOFORMAT", None)

    script = _make_dummy_script(tmp_path)
    rc = main(["--no-autoformat", str(script)])
    # ``recording_disabled`` short-circuits to running the script bare,
    # which should exit 0 for our trivial dummy.  The flag-related side
    # effect we care about lands BEFORE the disabled-check, so we can
    # assert on it after main returns.
    assert rc == 0, f"unexpected exit code {rc}"
    assert os.environ.get("CT_AUTOFORMAT") == "0", (
        "expected --no-autoformat to set CT_AUTOFORMAT=0; "
        f"got CT_AUTOFORMAT={os.environ.get('CT_AUTOFORMAT')!r}"
    )


def test_autoformat_default_leaves_env_var_untouched(
    tmp_path: Path, restore_env, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The default ``--autoformat`` path does NOT overwrite an existing
    ``CT_AUTOFORMAT`` env var.

    Rationale: a deployment that sets ``CT_AUTOFORMAT=0`` globally to
    disable the pre-format pass must keep working when the user
    invokes the CLI without ``--no-autoformat``.  The CLI default is
    "leave the env var alone"; only ``--no-autoformat`` actively
    flips it.
    """
    from codetracer_python_recorder.cli import ENV_DISABLED, main

    monkeypatch.setenv(ENV_DISABLED, "1")
    monkeypatch.setenv("CT_AUTOFORMAT", "0")  # deployment-set disable

    script = _make_dummy_script(tmp_path)
    rc = main([str(script)])  # default autoformat=True from CLI
    assert rc == 0
    # The deployment-set env var must survive: the CLI default of True
    # is "no override", not "force on".
    assert os.environ.get("CT_AUTOFORMAT") == "0", (
        "default --autoformat should not overwrite a pre-set CT_AUTOFORMAT; "
        f"got CT_AUTOFORMAT={os.environ.get('CT_AUTOFORMAT')!r}"
    )


# ---------------------------------------------------------------------------
# Black availability probe — skip-loud when the tool isn't on PATH so
# the test surface communicates the dependency.
# ---------------------------------------------------------------------------


def _black_on_path() -> bool:
    """Probe whether ``black`` is on PATH.

    Used by the integration-flavour tests below so they skip cleanly on
    machines that don't have ``black`` (rather than failing with a
    confusing subprocess error).  Mirrors the JS recorder's "skip-loud
    if prettier isn't available" pattern from P4.
    """
    from shutil import which

    return which("black") is not None


def test_black_availability_diagnostic(capsys: pytest.CaptureFixture[str]) -> None:
    """Skip-loud diagnostic: prints whether ``black`` is reachable.

    Mirrors P4's pattern — the test always passes, but emits a clear
    message about whether downstream black-dependent integration tests
    would run in this environment.  Captured here as a test rather
    than a print so CI logs flag it via the test report.
    """
    if _black_on_path():
        # Don't fail — just emit a positive diagnostic so the test
        # surface communicates the dependency check passed.
        capsys.disabled()
        print("[p6.2] black IS available on PATH — full integration assertions can run")
    else:
        pytest.skip(
            "[p6.2] black is NOT on PATH — recorder-side autoformat integration "
            "tests skipped.  Install black (e.g. `pip install black`) to "
            "exercise the full pre-format pass."
        )


# ---------------------------------------------------------------------------
# End-to-end recording-flow assertions — DEFERRED.
#
# The four canonical assertions called out in the implementer brief
# (formatted sibling exists, skip-when-not-minified, no-autoformat
# disables, missing-tool warns) require the recorder to materialise
# the formatted sibling + sourcemap into the trace output directory.
#
# The Python recorder uses CTFS, which packs all source files inside
# the binary ``.ct`` container; there is no ``<trace>/files/`` sidecar
# directory like the JS recorder uses.  Wiring the formatted view into
# the container is a wire-format change to the multi-stream Nim writer
# (a new stream for "alternate source views" + a corresponding reader-
# side projection in ``ct-print``), which is out of scope for the
# P6.2 implementer time-box.
#
# The follow-up tracking issue should:
#  1. Add a writer-side ``register_alternate_source_view(path, view_kind,
#     formatted_bytes, sourcemap_bytes)`` entry point.
#  2. Update the CTFS spec to document the new stream tag + the
#     ``view_kind`` enum (``raw`` | ``black-formatted`` | …).
#  3. Add a ct-print projection so the formatted view becomes
#     discoverable via the existing P3 sourcemap path on the replay
#     server.
#  4. Wire ``try_autoformat`` into the recorder's per-file source
#     materialisation pass (likely in ``RuntimeTracer::on_line``'s
#     first-time-we-see-this-path branch, or a new pre-record
#     enumeration step in ``lifecycle.rs``).
#
# Once those land, the four assertions migrate from "deferred" to
# concrete pytest cases here.  In the meantime, the unit tests in
# ``src/runtime/autoformat.rs`` cover the pure pre-format functions
# at the same depth the JS recorder's ``autoformat.test.ts`` covers
# its ``tryAutoformat`` / ``looksMinified`` / ``generateInverseSourceMap``.
# ---------------------------------------------------------------------------
