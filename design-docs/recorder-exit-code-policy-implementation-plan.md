# Recorder Exit Code Policy – Implementation Plan

Plan owners: codetracer Python recorder maintainers  
Related ADR: 0017 – Recorder Exit Code Policy  
Target release: codetracer-python-recorder 0.x (next minor)

## Goals
- Default the recorder CLI to exit with status `0` when tracing succeeds, even if the target script exits non-zero.
- Preserve the script exit status in trace metadata and surface it through logs so users stay informed.
- Provide consistent configuration knobs (CLI flag, environment variable, policy API) to re-enable exit-code passthrough when desired.
- Ensure recorder failures (`start`, `flush`, `stop`, `require_trace`) still emit non-zero exit codes.

## Non-Goals
- Changing how `ct record` parses or surfaces recorder output beyond the new default.
- Altering metadata schemas storing script exit information.
- Introducing scripting hooks for arbitrary exit-code transforms outside passthrough vs. success modes.

## Current Gaps
- `codetracer_python_recorder.cli.main` (`codetracer_python_recorder/cli.py:165`) always returns the traced script's exit code; there is no concept of recorder success vs. script result.
- `RecorderPolicy` (`src/policy/model.rs`) and associated FFI lack an exit-code behaviour flag, so env policy and embedding APIs cannot control the outcome.
- No CLI argument or environment variable communicates the user's preference, and the help text/docs imply passthrough semantics.
- There are no regression tests asserting CLI exit behaviour; `ct record` integration relies on current passthrough behaviour implicitly.

## Workstreams

### WS1 – Policy & Configuration Plumbing
**Scope:** Extend recorder policy models and configuration surfaces with an exit-code behaviour flag.
- Add `propagate_script_exit_code: bool` to `RecorderPolicy` (default `false`) plus matching field in `PolicyUpdate`; update `apply_update`/`Default` implementations.
- Extend PyO3 bindings `configure_policy_py`/`policy_snapshot` to accept and expose `propagate_script_exit_code`.
- Add environment variable `CODETRACER_PROPAGATE_SCRIPT_EXIT` in `policy/env.rs`, parsing booleans via existing helpers.
- Update Python session helpers (`session.py`) to pass through a `propagate_script_exit` policy key, including `_coerce_policy_kwargs`.
- Unit tests:
  - Rust: policy default and update round-trips (`policy::model` & `policy::ffi` tests).
  - Rust: env configuration toggles new flag and rejects invalid values.
  - Python: `configure_policy` keyword argument path accepts the new key.

### WS2 – CLI Behaviour & Warning Surface
**Scope:** Teach the CLI to honour the new policy and compute the final exit status.
- Introduce `--propagate-script-exit` boolean flag (default `False`) wired into CLI help; set `policy["propagate_script_exit"] = True` when provided.
- After `start(...)`, cache the effective propagation flag by inspecting CLI config and, when unspecified, consulting `policy_snapshot()` to honour env defaults.
- Rework `main`'s shutdown path:
  - Track recorder success across `start`, script execution, `flush`, and `stop`.
  - Decide final process exit: return `script_exit` if propagation enabled, otherwise `0` when recorder succeeded; use a distinct error code (existing) when recorder fails.
  - On non-zero script exit with propagation disabled, emit a concise stderr warning mentioning the exit status and `--propagate-script-exit`.
- Ensure the script exit status continues to flow into `stop(exit_code=...)` and metadata serialisation unchanged.
- Add CLI unit/integration tests (pytest) covering combinations: default non-propagating success/failure, `--propagate-script-exit`, and recorder failure paths (e.g., missing script).

### WS3 – Documentation, Tooling, and Release Notes
**Scope:** Update user-facing materials and automation checks.
- Refresh CLI `--help`, README, and docs (`docs/book/src/...` if applicable) to describe default exit behaviour and configuration options.
- Document `CODETRACER_PROPAGATE_SCRIPT_EXIT` and Python policy key in API guides.
- Add CHANGELOG entry summarising behaviour change and migration guidance for users relying on passthrough.
- Extend CI/test harness:
  - Add regression test via `just test` hitting CLI exit codes (likely in Python test suite under `tests/`).
  - Update any existing `ct record` integration smoke tests to pin the expected default (0) where relevant.
- Coordinate with desktop CLI maintainers to flip their expectations once the recorder release lands.

## Timeline & Dependencies
- WS1 should land first to provide configuration plumbing for CLI work.
- WS2 depends on WS1's policy flag; both should merge within the same feature branch to avoid transient inconsistent behaviour.
- WS3 can progress in parallel once WS2 stabilises, but final doc updates should wait for CLI flag names to settle.
