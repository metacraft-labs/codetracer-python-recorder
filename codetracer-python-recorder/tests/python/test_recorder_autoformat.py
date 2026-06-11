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
# End-to-end recording-flow assertions.
#
# The recorder-side autoformat hook fires from
# ``src/runtime/tracer/events.rs::on_line`` (first sighting of a path)
# and ``src/runtime/output_paths.rs::configure_writer`` (the activation
# path).  Both call sites invoke ``maybe_register_autoformat_view``
# which runs ``try_autoformat`` once per source path and, on a
# successful outcome, calls the writer's ``register_source_view`` FFI
# entry point.  The CTFS writer buffers the view in memory and emits
# it into ``source_views.dat`` / ``source_views.off`` on close, setting
# ``FLAG_HAS_ALTERNATE_SOURCE_VIEWS`` (bit 5) on ``meta.dat``.
#
# The tests below record real Python scripts and decode the resulting
# ``.ct`` container via ``ct-print --full`` to assert on the
# ``metadata.flags.has_alternate_source_views`` bit + the
# ``source_views[]`` array surfaced by the canonical Nim reader.
# ---------------------------------------------------------------------------


# A canonical minified Python source: many statements packed on a
# single line.  Average line length comfortably above the
# ``DEFAULT_MINIFIED_THRESHOLD`` of 500 chars so the
# ``looks_minified`` heuristic triggers, then ``black`` reformats
# the bundle onto separate lines (forcing a strict ``fmt_line_count >
# orig_line_count`` check inside ``try_autoformat`` to pass).
#
# Each statement uses a distinct, identifier-like name so the
# inverse-sourcemap's anchor-token logic (in
# ``generate_inverse_sourcemap``) finds at least one unique anchor per
# original line — this keeps the generated sourcemap non-degenerate
# and exercises the full V3 VLQ encoding path the Rust unit tests
# already cover at the function level.
_MINIFIED_FIXTURE = (
    "alpha_one=1; beta_two=2; gamma_three=alpha_one+beta_two; "
    "delta_four=gamma_three*gamma_three; "
    "epsilon_five=delta_four-beta_two; "
    "zeta_six=epsilon_five+alpha_one; "
    "eta_seven=zeta_six*beta_two; "
    "theta_eight=eta_seven-alpha_one; "
    "iota_nine=theta_eight+gamma_three; "
    "kappa_ten=iota_nine*delta_four; "
    "lambda_eleven=kappa_ten+epsilon_five; "
    "mu_twelve=lambda_eleven*zeta_six; "
    "nu_thirteen=mu_twelve-eta_seven; "
    "xi_fourteen=nu_thirteen+theta_eight; "
    "omicron_fifteen=xi_fourteen*iota_nine; "
    "pi_sixteen=omicron_fifteen+kappa_ten; "
    "rho_seventeen=pi_sixteen*lambda_eleven; "
    "sigma_eighteen=rho_seventeen-mu_twelve; "
    "tau_nineteen=sigma_eighteen+nu_thirteen; "
    "upsilon_twenty=tau_nineteen*xi_fourteen; "
    "print(upsilon_twenty)\n"
) * 3  # repeat to keep avg line length comfortably above the threshold


def _ct_print_full(ct_path: Path) -> dict:
    """Decode *ct_path* with ``ct-print --full`` and return the bundle.

    Thin wrapper around the suite-wide ``support.ctfs.ct_print_full``
    helper — duplicated here so this test file stays self-contained
    when grepped for the autoformat tests.
    """
    from .support.ctfs import ct_print_full

    return ct_print_full(ct_path)


def _record(tmp_path: Path, source: str, *, name: str = "fixture.py") -> Path:
    """Record *source* through the public recorder API and return the
    resulting ``.ct`` container path.

    Wrapper around ``support.ctfs.record_script`` to keep the test
    bodies focused on the assertions rather than the recording
    boilerplate.
    """
    from .support.ctfs import record_script

    script = tmp_path / name
    script.write_text(source, encoding="utf-8")
    trace_dir = tmp_path / "trace"
    return record_script(trace_dir, script)


def test_p6_2_py_recorder_emits_source_view_for_minified_input(tmp_path: Path) -> None:
    """P6.2: recording a minified Python source materialises a
    ``black``-formatted alternate view inside the CTFS container.

    End-to-end acceptance for the recorder-side autoformat hook:
    ``RuntimeTracer::on_line`` (and ``TraceOutputPaths::configure_writer``
    for the activation path) calls ``maybe_register_autoformat_view``
    on the first sighting of each source path; the helper runs
    ``try_autoformat`` and on a successful outcome forwards the
    formatted source + V3 sourcemap to the writer's
    ``register_source_view`` FFI entry point.  The CTFS writer emits
    ``source_views.dat`` / ``source_views.off`` on close and flips
    ``FLAG_HAS_ALTERNATE_SOURCE_VIEWS`` (bit 5) on ``meta.dat``.

    Strict assertions:

    * ``metadata.flags.has_alternate_source_views`` is ``True`` — the
      reader surfaces the meta.dat flag bit, and it MUST be set on a
      trace that successfully buffered at least one view.
    * ``counts.source_views >= 1`` — the source_views interning table
      carries at least one entry.
    * At least one ``source_views[i]`` entry has ``view_kind == 2``
      (the spec's ``black_format`` constant) and ``view_name`` ending
      in ``.fmt.py`` (the recorder's naming convention, matching the
      JS recorder's sibling-discovery pattern).

    Skipped-loud when ``black`` isn't on PATH so the test surface
    documents the dependency — install ``black`` (``pip install black``)
    to exercise the full integration.
    """
    if not _black_on_path():
        pytest.skip(
            "[p6.2] black is NOT on PATH — recorder-side autoformat integration "
            "tests skipped.  Install black (e.g. `pip install black`) to "
            "exercise the full pre-format pass."
        )

    ct_path = _record(tmp_path, _MINIFIED_FIXTURE, name="bundle.min.py")
    bundle = _ct_print_full(ct_path)

    metadata = bundle.get("metadata", {})
    flags = metadata.get("flags", {})
    assert flags.get("has_alternate_source_views") is True, (
        "meta.dat must carry FLAG_HAS_ALTERNATE_SOURCE_VIEWS (bit 5) when "
        "the recorder successfully buffered at least one alternate source "
        f"view; got flags={flags}.  Likely regression: the "
        "`maybe_register_autoformat_view` hook in events.rs / "
        "output_paths.rs is no longer forwarding the formatted view to "
        "`TraceWriter::register_source_view`, or the writer's on-close "
        "path is no longer flipping the meta.dat flag bit."
    )

    counts = bundle.get("counts", {})
    assert int(counts.get("source_views", 0)) >= 1, (
        "counts.source_views must be >=1 after a minified recording; got "
        f"counts={counts}.  Likely regression: ``try_autoformat`` is "
        "returning ``Skipped`` instead of ``Ok``, or the writer is "
        "dropping the buffered view at close time."
    )

    source_views = bundle.get("source_views", [])
    black_views = [
        sv for sv in source_views
        if int(sv.get("view_kind", -1)) == 2
        and str(sv.get("view_name", "")).endswith(".fmt.py")
    ]
    assert black_views, (
        "no source_views entry carries the canonical ``view_kind == 2`` "
        "(black_format) + ``.fmt.py`` view_name pair; got source_views="
        f"{source_views}.  Likely regression: the recorder is now emitting "
        "a different view_kind (perhaps the spec's ``prettier_format``=1 "
        "by mistake) or naming the view something other than "
        "``<stem>.fmt.py``."
    )


def test_p6_2_py_recorder_no_source_view_for_normal_source(tmp_path: Path) -> None:
    """P6.2: recording a normal (non-minified) Python source does NOT
    emit a source view.

    Negative-case companion to the minified-input test above.  Steady-
    state hand-written code lands on ``SkipReason::NotMinified`` inside
    ``try_autoformat`` (the heuristic short-circuits before invoking
    ``black``), and the recorder leaves the meta.dat flag clear +
    ``source_views.dat`` absent.

    Strict assertions:

    * ``metadata.flags.has_alternate_source_views`` is ``False`` — the
      flag bit MUST NOT be set when no view was buffered.
    * ``counts.source_views == 0`` — the source_views interning table
      MUST be empty.

    This test does NOT depend on ``black`` being on PATH — the
    heuristic short-circuits before the tool invocation, so the
    negative outcome is the same whether or not ``black`` is
    installed.
    """
    normal_source = (
        "def main():\n"
        "    x = 1\n"
        "    y = 2\n"
        "    z = x + y\n"
        "    print(z)\n"
        "\n"
        "main()\n"
    )

    ct_path = _record(tmp_path, normal_source, name="normal.py")
    bundle = _ct_print_full(ct_path)

    metadata = bundle.get("metadata", {})
    flags = metadata.get("flags", {})
    assert flags.get("has_alternate_source_views") is False, (
        "meta.dat must NOT carry FLAG_HAS_ALTERNATE_SOURCE_VIEWS when "
        "the recorder buffered no alternate source views; got "
        f"flags={flags}.  Likely regression: the writer is flipping the "
        "flag bit unconditionally instead of gating on "
        "``viewCount > 0`` at close time."
    )

    counts = bundle.get("counts", {})
    assert int(counts.get("source_views", 0)) == 0, (
        "counts.source_views must be 0 after a non-minified recording; "
        f"got counts={counts}.  Likely regression: ``try_autoformat`` is "
        "no longer short-circuiting on ``looks_minified`` and is "
        "spuriously emitting views for hand-written sources."
    )
