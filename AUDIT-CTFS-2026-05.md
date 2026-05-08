# Python Recorder CTFS Audit — 2026-05-08

This audit checks `codetracer-python-recorder` against the CTFS-only
contract of `codetracer-specs/Recorder-CLI-Conventions.md` §4 (CTFS-only
output) and §5 (env-var fallbacks).  The 2026-05-02 round of recorder
audits established the CTFS-default pattern across Cairo, Cardano,
Circom, Flow, Fuel and EVM recorders; the 2026-05-08 follow-up brought
the JavaScript recorder into compliance.  This entry documents the
analogous follow-up for the Python recorder.

The Python recorder is a Rust-backed PyO3 extension wrapped by a thin
Python package:

* `codetracer_python_recorder/` — Python wrapper package.  Owns the
  CLI entry point (`cli.py`, registered as the `codetracer-python-recorder`
  console script and as `python -m codetracer_python_recorder`), the
  library-mode auto-start hook (`auto_start.py`), the Python session
  helpers (`session.py`, `api.py`), and the format constants
  (`formats.py`).
* `src/` — Rust source for the PyO3 extension built with maturin.
  Bridges to the shared `codetracer_trace_writer_nim` crate from
  `codetracer-trace-format-nim`, which provides the canonical CTFS
  on-disk container.
* `crates/recorder-errors` — structured-error catalog shared with the
  PyO3 extension.
* `tests/python/` — Python integration and unit tests.
* `tests/rust*` — Rust unit tests built behind the `integration-test`
  feature.

The recorder uses the **PyO3 + Nim writer** path: every event captured
by `sys.monitoring` flows through the Rust runtime tracer in
`src/runtime/tracer/` and is serialised by the shared Nim writer.

## Convention compliance follow-up — 2026-05-08

Pre-2026-05-08 the Python recorder accepted a `--format
binary|ctfs|json` flag on the CLI and read `CODETRACER_FORMAT` from the
auto-start environment.  When `--format json` was active the recorder
wrote a `trace.json` events sidecar instead of the canonical CTFS
`.ct` container.

Subsequent to the 2026-05-02 audits, `Recorder-CLI-Conventions.md` §4
in `codetracer-specs` was tightened to require **CTFS-only** output:
recorders no longer accept a `--format` flag and `ct print` (shipped
with `codetracer-trace-format-nim`) is the canonical conversion tool
for human-readable output.  `Repo-Requirements.md` §2.2 / §2.3 reflect
this contract.

This entry records the convention-compliance changes applied to the
Python recorder on 2026-05-08:

### CLI surface

* `--format` was removed from `codetracer_python_recorder/cli.py`
  (lines ~65-72 in the pre-change file).  An explicit guard now
  rejects `--format` (and the collapsed `--format=value` form) with
  argparse exit code 2 and a stderr message that points users at
  `ct print` for human-readable conversion.
* `--out-dir` / `-o` is now backed by the env-var fallback
  `CODETRACER_PYTHON_RECORDER_OUT_DIR` per convention §5.  The
  helper `cli.resolve_out_dir(...)` encapsulates the precedence
  chain: CLI flag → env var → `./trace-out`.
* `cli.recording_disabled()` reads `CODETRACER_PYTHON_RECORDER_DISABLED`.
  When truthy (`1` or `true`, case-insensitive) the CLI runs the
  target script / pytest / unittest invocation **without** starting
  the recorder.  No trace artefacts are produced; the target's exit
  code is preserved.  This mirrors the JS recorder's
  `CODETRACER_JS_RECORDER_DISABLED`.
* The `--help` output now documents the env-var fallbacks, points
  users at `ct print` for human-readable conversion, and states
  explicitly that the recorder always writes the canonical CTFS
  multi-stream container.
* `--version` (already present) was promoted into the convention
  audit so the verifier can lock it in.

### Auto-start surface

* `codetracer_python_recorder/auto_start.py` no longer reads
  `CODETRACER_FORMAT`.  The constant `ENV_TRACE_FORMAT` was removed
  from the module and from `__all__`.  The format passed to
  `session.start(...)` is hard-pinned to `formats.DEFAULT_FORMAT`
  (i.e. `"ctfs"`).
* `CODETRACER_TRACE` and `CODETRACER_TRACE_FILTER` (the auto-start
  path env vars) are unchanged — they govern *whether* tracing
  starts, not *what format* it writes.

### Native / writer surface

* The Rust `TraceEventsFileFormat::Json` variant lives in the shared
  `codetracer_trace_writer_nim` crate and remains importable, but the
  CLI surface no longer routes user requests to it.  The CLI passes
  `"ctfs"` unconditionally through `session.start(...)` →
  `start_tracing` (PyO3 boundary) → `TraceSessionBootstrap::prepare`
  → `resolve_trace_format`.  Users who need JSON for golden-snapshot
  fixtures run `ct print --json trace.ct`.
* `src/runtime/output_paths.rs::TraceOutputPaths::new` continues to
  produce `trace.ct` for the CTFS variant.  The other branches
  (`Json`, `Binary`, `BinaryV0`) remain reachable from internal
  Python-API users (`session.start(format=...)`); they exist to
  support the legacy event-stream content-assertion tests in
  `tests/python/test_monitoring_events.py` that read the JSON event
  array directly.  Those tests are *not* user-facing CLI contracts;
  no end-user CLI invocation routes there.
* `src/session/bootstrap/filesystem.rs::resolve_trace_format` is
  unchanged; its tests still cover all four format strings so an
  accidental drop of a variant is caught at the Rust layer.

### Test rewrites

Tests that previously asserted on `--format json`-produced
`trace.json` content fall into two groups:

* **CLI tests** (the user-facing contract).  All `--format json`
  invocations were dropped from `tests/python/test_cli_integration.py`
  and the assertions were rewritten against the canonical CTFS
  `trace.ct` container.  The new test
  `test_recorded_trace_via_ct_print_json` records a small Python
  script and pipes the produced `trace.ct` through
  `codetracer-trace-format-nim/ct-print --json`, asserting on
  structural anchors (script filename, function names, variable
  names) rather than integer values — the
  cardano/circom/flow/fuel/leo/miden/move/polkavm precedents
  document that integer values may not round-trip through every
  encoder path, so we deliberately do not assert on them.
* **Internal API tests** (`test_monitoring_events.py`,
  `test_exit_payloads.py`, `test_trace_balance.py`).  These tests
  call `codetracer.start(format='json')` directly to inspect the raw
  JSON event stream.  They are not CLI contracts and the JSON path
  remains available at the Python API layer for them.  The single
  CLI-driven test in `test_exit_payloads.py` was rewritten to drive
  the recorder via the Python API in a subprocess, preserving the
  event-content assertions while keeping the CLI itself CTFS-only.

Files modified by the rewrite (CLI flag stripping):

* `tests/python/test_cli_integration.py` — full rewrite around the
  CTFS-only contract.  Adds the convention tests:
  `test_format_flag_rejected`, `test_no_format_flag_in_help`,
  `test_help_mentions_ct_print`, `test_env_out_dir_used_when_flag_omitted`,
  `test_cli_flag_overrides_env_out_dir`, `test_env_disabled_skips_recording`,
  and the ct-print round-trip `test_recorded_trace_via_ct_print_json`.
* `tests/python/test_pytest_integration.py` — strips `--format json`
  from 13 invocations; updates the trace-file existence check to
  look for `trace.ct` and explicitly forbids `trace.json`.
* `tests/python/test_policy_runtime.py` — strips `--format json`
  from 4 invocations; updates the trace-file existence check.
* `tests/python/test_exit_payloads.py` — switches from CLI mode to
  Python-API mode (driven via subprocess) so the `Return` payload
  shape can still be asserted on the JSON event stream.
* `tests/python/test_hcr.py` — strips the redundant `--format ctfs`
  arguments now that CTFS is the only format.
* `tests/python/unit/test_cli.py` — replaces
  `test_parse_args_validates_format` with `test_parse_args_rejects_format_flag`;
  adds tests for `resolve_out_dir(...)` and `recording_disabled()`.

### New tests added

The convention-compliance tests live alongside the rest of the CLI
integration suite in `tests/python/test_cli_integration.py`:

* `test_format_flag_rejected` — `--format json|binary|ctfs` and
  `--format=json` all exit non-zero with a `--format` mention in
  stderr.
* `test_no_format_flag_in_help` — `--help` must not advertise
  `--format` or `CODETRACER_FORMAT`.
* `test_help_mentions_ct_print` — `--help` must point users at
  `ct print` for human-readable conversion.
* `test_env_out_dir_used_when_flag_omitted` /
  `test_cli_flag_overrides_env_out_dir` — both directions
  (env-only and flag-wins-over-env).
* `test_env_disabled_skips_recording` — `=1` and `=TRUE` both skip
  recording; the target script still executes; no trace artefacts
  are produced.
* `test_recorded_trace_via_ct_print_json` — record a real Python
  script, pipe through `ct-print --json`, assert structural anchors.

The shell-level guard at
`codetracer-python-recorder/tests/verify-cli-convention-no-silent-skip.sh`
runs from `just test`/`just lint` and the `pytest` test scripts.  It
asserts the canonical strings are present in `--help`, the legacy
strings are absent, the env-var references survive in source, and
`auto_start.py` no longer mentions `CODETRACER_FORMAT`.

### Files modified / added

Modified:

* `codetracer-python-recorder/codetracer_python_recorder/cli.py`
* `codetracer-python-recorder/codetracer_python_recorder/auto_start.py`
* `codetracer-python-recorder/README.md`
* `codetracer-python-recorder/Justfile` (verify-cli-convention wiring)
* `codetracer-python-recorder/pyproject.toml` (verify-cli-convention test script)
* `codetracer-python-recorder/tests/python/test_cli_integration.py`
* `codetracer-python-recorder/tests/python/test_pytest_integration.py`
* `codetracer-python-recorder/tests/python/test_policy_runtime.py`
* `codetracer-python-recorder/tests/python/test_exit_payloads.py`
* `codetracer-python-recorder/tests/python/test_hcr.py`
* `codetracer-python-recorder/tests/python/unit/test_cli.py`
* `codetracer-specs/Recorder-CLI-Conventions.md` (Implementation
  Status table; §3 / §5 current-state-vs-convention rows)

Added:

* `codetracer-python-recorder/tests/verify-cli-convention-no-silent-skip.sh`
* `AUDIT-CTFS-2026-05.md` (this file)

### Known coverage regression — trace-filter chain assertions (follow-up)

`test_cli_honours_trace_filter_chain` and `test_cli_honours_env_trace_filter`
in `tests/python/test_cli_integration.py` were softened to smoke tests
as part of this work.  This is an explicit, documented coverage gap —
not a silent weakening — and should be revisited.

**What the original tests asserted.**  Pre-2026-05 both tests parsed
the `--format json` sidecar `trace_metadata.json` and verified the
exact ordered list of trace-filter file paths recorded under
`trace_filter.filters[].path`:

* `test_cli_honours_trace_filter_chain` asserted that an explicit
  `--trace-filter override-filter.toml` plus the implicit
  `.codetracer/trace-filter.toml` discovery yielded the chain
  `["<inline:builtin-default>", default_filter, override_filter]`.
* `test_cli_honours_env_trace_filter` asserted that
  `CODETRACER_TRACE_FILTER=env-filter.toml` yielded the chain
  `["<inline:builtin-default>", env_filter]`.

These assertions verified **filter-chain *configuration*** — i.e.
which TOML files the recorder loaded — not filter-chain *effect*
on the recorded event stream.

**Why we cannot replace them with `ct print` assertions.**  The
canonical CTFS conversion tool (`ct print --json`,
`ct print --summary`) exposes the *post-filter* paths / functions /
steps actually recorded, but does not expose the metadata sidecar
contents (e.g. the ordered list of filter-file paths the recorder
loaded).  Empirically (verified 2026-05-08 against
`codetracer-trace-format-nim/ct-print` from the workspace), the
specific filter content used by the original tests
(`selector = "pkg:program"` + `exec = "skip"`) does not visibly
change the recorded `paths` array for a single-script invocation
like the one the original tests exercised, so we cannot derive an
equivalent post-filter assertion through `ct print` either.

**What was retained.**  The smoke tests still verify that `--trace-filter`
and `CODETRACER_TRACE_FILTER` are accepted by the CLI without error
and that a CTFS container is produced — i.e. the recorder doesn't
crash on a valid filter file and doesn't refuse to load the env-var
path.  This catches the most common breakage modes (TOML parser
regressions, env-var reading regressions, CLI plumbing regressions).

**What is now uncovered at the CLI layer.**

1. The exact loaded filter chain (inline builtin → discovered
   `.codetracer/trace-filter.toml` → explicit `--trace-filter` arg)
   ordering and identity.
2. The `CODETRACER_TRACE_FILTER` env var being honoured *as a filter
   source* (vs being silently ignored): a recorder that ignores the
   env var entirely would still pass the current smoke test.

**Follow-up options to revisit (tracked here, not yet scheduled).**

* **Option A — extend `ct print`.**  Teach
  `codetracer-trace-format-nim/ct-print` to surface the embedded
  metadata stream (recorder name, args, trace-filter chain) under a
  new flag (e.g. `ct print --metadata trace.ct`).  Then strengthen
  both tests to assert on that output.  This is the structurally
  correct fix per convention §4 and aligns with the cardano /
  circom / flow / fuel / leo / miden / move / polkavm precedent of
  using `ct print` for all post-recording inspection.
* **Option B — assert through filter effect.**  Construct a filter
  chain whose *effect* is observable in `ct print --summary` (e.g.
  a filter that excludes a known third-party module, then verify
  that module does not appear in the `paths`/`functions` arrays of
  the recorded trace).  This requires a more elaborate test fixture
  but does not need any change to `ct print`.
* **Option C — trace-filter unit tests.**  Move the chain-loading
  assertion into a Python-API-level test that calls
  `codetracer_python_recorder.session.start(...)` directly and
  inspects the resulting policy snapshot.  This bypasses the CLI
  contract layer but verifies the same code path that the CLI
  invokes internally.

The right choice is probably Option A: it gives every recorder a
uniform way to inspect its own trace-filter chain through the
canonical conversion tool.  Tracked as a future task in this
follow-up section; no due date assigned.

### References

* [`codetracer-specs/Recorder-CLI-Conventions.md`](../codetracer-specs/Recorder-CLI-Conventions.md) §4 (CTFS-only) and §5 (env vars).
* [`codetracer-specs/Repo-Requirements.md`](../codetracer-specs/Repo-Requirements.md) §2.2 (CLI compliance) and §2.3 (trace format compatibility).
* JS precedent: `codetracer-js-recorder/AUDIT-CTFS-2026-05.md`.
* Cairo precedent: `codetracer-cairo-recorder` commit 2710b5e
  ("Recorder convention compliance: drop --format, add CTFS-only contract").
