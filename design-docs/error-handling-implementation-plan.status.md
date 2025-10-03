# Error Handling Implementation Plan — Status

_Last updated: 2025-10-04_

## WS1 – Foundations & Inventory
State: In progress
Tooling: `just errors-audit` (finds `PyRuntimeError::new_err`, `unwrap`/`expect`/`panic!`, Python `RuntimeError`/`ValueError`).
What we saw:
- Rust modules now emit `RecorderError`; raw Python exceptions survive in `codetracer_python_recorder/session.py` and tests (ISSUE-014).
- `src/monitoring/tracer.rs` still uses `lock().unwrap()` and lacks error reporting for callback failures (ISSUE-013).
- Python glue keeps legacy assertions/unwraps (ISSUE-012).
Next moves:
- Land ISSUE-013 to sort the locking story.
- Plan the Python facade cleanup (ISSUE-014) once WS4 is steady.

## WS2 – `recorder-errors` Crate
State: Done (2025-10-02)
Highlights:
- Added `crates/recorder-errors` with `RecorderError`, enums, context helpers, macros (`usage!`, `enverr!`, `target!`, `bug!`, `ensure_*`), plus tests and optional serde support.
- `cargo test -p recorder-errors` + workspace `cargo check` stay green.
Next moves: Use this crate everywhere in WS3/WS4 work.

## WS3 – Retrofit Rust Modules
State: Done (2025-10-02)
Highlights:
- `session/*`, `runtime/*`, and `monitoring/tracer.rs` now return `RecorderError` via the shared macros.
- Python exposure happens through one `errors` mapper; IO errors now carry context.
- No stray `PyRuntimeError::new_err` left outside that mapper.
Next moves: Feed findings into WS4 and loop back to WS1 issues.

## WS4 – FFI Wrapper & Python Exception Hierarchy
State: Done (2025-10-02)
Highlights:
- Added `ffi` guard around each PyO3 entry point to map `RecorderError` plus panic safety.
- Exposed Python classes `RecorderError`, `UsageError`, `EnvironmentError`, `TargetError`, `InternalError`.
- Rust and Python tests cover the new flow (`uv run cargo nextest run ...`; `.venv/bin/python -m pytest ...`).
Next moves: Hold for WS5 until ISSUES 013/014 close.

## WS5 – Policy Switches & Runtime Configuration
State: Done (2025-10-03)
Highlights:
- `TraceSession.start()` and `trace()` now refresh policy from env vars and accept override mappings so embeds wire recorder switches without manual plumbing.
- Rust exports expose `configure_policy`/`configure_policy_from_env` under the expected Python names; unit tests cover env-driven and explicit override flows.
- Runtime tracer finish path honours `RecorderPolicy`: callback errors respect `on_recorder_error` (disable detaches without surfacing exceptions), `require_trace` now fails cleanly when no events land, and partial traces are deleted or retained based on `keep_partial_trace`.
- Python CLI integration tests exercise disable vs abort paths and require-trace enforcement using the new failure-injection toggles; CLI now propagates runtime shutdown errors so exit codes reflect policy outcomes while partial traces are cleaned per configuration.
Next moves: Kick off WS6 once upstream WS1 cleanups land.

## WS6 – Logging, Metrics, and Diagnostics
State: Done (2025-10-03)
Highlights:
- Replaced the `env_logger` helper with a structured JSON logger that always emits `run_id`, active `trace_id`, and `error_code` fields while honouring policy-driven log level and log file overrides.
- Introduced a pluggable `RecorderMetrics` sink and instrumented dropped locations, policy-triggered detachments, and caught panics across the monitoring/runtime paths; Rust unit tests exercise the metrics capture.
- Enabled the `--json-errors` policy path so runtime shutdown emits a single-line JSON trailer on stderr; CLI integration tests now assert the abort flow surfaces the trailer alongside existing stack traces.
Next moves: Wire the metrics sink into the chosen exporter and align the log schema with Observability consumption before rolling out to downstream tooling.

## WS7 – Test Coverage & Tooling Enforcement
State: Done (2025-10-04)
Highlights:
- Expanded `recorder-errors` and policy unit tests covering every macro (usage/target/internal ensures) plus invalid boolean parsing.
- Added FFI unit tests for `dispatch`/`wrap_pyfunction`, panic containment, and Python exception attribute propagation.
- Introduced integration coverage for environment permission failures, injected target argument capture errors, and synthetic callback panics (verifying JSON trailers and error classes).
- Implemented `just lint` orchestration running `cargo clippy -D clippy::panic` and a repository script that blocks unchecked `.unwrap(` usage outside the legacy allowlist.
Next moves: Monitor unwrap allowlist shrinkage once WS1 follow-ups land; evaluate extending the lint to `.expect(` once monitoring refactor closes.

## Upcoming Workstreams
WS8 – Documentation & Rollout: Not started. Pending guidance from Docs WG and ADR promotion once downstream consumers validate the new error interfaces.
