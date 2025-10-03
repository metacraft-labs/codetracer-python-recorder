# Error Handling Implementation Plan — Status

_Last updated: 2025-10-02_

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
State: In progress
Highlights:
- `TraceSession.start()` and `trace()` now refresh policy from env vars and accept override mappings so embeds wire recorder switches without manual plumbing.
- Rust exports expose `configure_policy`/`configure_policy_from_env` under the expected Python names; unit tests cover env-driven and explicit override flows.
Next moves:
- Thread policy decisions into runtime tracer shutdown (detach vs abort) and partial-trace handling before promoting WS5 to done.

## Upcoming Workstreams
WS6–WS8: Not started. Blocked on WS1 follow-ups and ADR sign-off.
