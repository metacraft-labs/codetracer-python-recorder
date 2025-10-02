# Error Handling Implementation Plan — Status

_Last updated: 2025-10-02_

## WS1 – Foundations & Inventory
- **State:** In progress
- **Audit tooling:** `just errors-audit` (adds line-numbered search for `PyRuntimeError::new_err`, `unwrap`/`expect`/`panic!`, and Python `RuntimeError`/`ValueError` raises).
- **Key findings:**
  - Core Rust modules now emit `RecorderError` instances; remaining direct Python exceptions are concentrated in the Python facade (`codetracer_python_recorder/session.py`) and related tests (ISSUE-014).
  - Monitoring plumbing still relies on `lock().unwrap()` in `src/monitoring/tracer.rs` and lacks structured errors for callback failures (ISSUE-013).
  - Workspace still contains legacy assertions/unwraps tied to Python-facing glue (tracking via ISSUE-012).
- **Follow-up tracking:** ISSUE-011 (session/bootstrap), ISSUE-012 (runtime), ISSUE-013 (monitoring/FFI), ISSUE-014 (Python facade).
- **Next actions:** Socialise the audit command with the team; prioritise locking strategy work (ISSUE-013) and plan Python facade migration under WS4.

## WS2 – `recorder-errors` Crate
- **State:** Completed (2025-10-02)
- **Deliverables:** Workspace now hosts `crates/recorder-errors` with `RecorderError`, classification enums, context helpers, macros (`usage!`, `enverr!`, `target!`, `bug!`, `ensure_*`), and unit tests (`cargo test -p recorder-errors`). The crate exposes optional serde support and README docs per ADR guidance.
- **Verification:** `cargo test -p recorder-errors` and `cargo check` run clean in the workspace.
- **Next actions:** Coordinate WS3 to migrate existing modules (`session`, `runtime`, `monitoring`) onto the new façade and replace direct `PyRuntimeError` usage.

## WS3 – Retrofit Rust Modules
- **State:** Completed (2025-10-02)
- **Deliverables:** `session/bootstrap.rs`, `session.rs`, `runtime/mod.rs`, `runtime/output_paths.rs`, `runtime/frame_inspector.rs`, `runtime/value_capture.rs`, and `monitoring/tracer.rs` now emit `RecorderError` values via `usage!`/`enverr!`, with a shared `errors` module translating them into Python exceptions. Added contextual metadata to IO failures and removed bespoke `PyRuntimeError` strings.
- **Verification:** `just cargo-test` succeeds (workspace `cargo nextest run`); grep confirms no remaining `PyRuntimeError::new_err` outside the conversion helper in `errors.rs`.
- **Next actions:** Start WS4 to introduce the FFI wrapper and Python exception hierarchy; continue WS1 by delegating ISSUE-013 (mutex handling) and ISSUE-014 (Python facade) owners.

## Upcoming Workstreams
- WS4–WS8 remain **Not started** pending completion of WS1 groundwork and ADR acceptance.
