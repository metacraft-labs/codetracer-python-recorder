# Error Handling Implementation Plan — Status

_Last updated: 2025-10-02_

## WS1 – Foundations & Inventory
- **State:** In progress
- **Audit tooling:** `just errors-audit` (adds line-numbered search for `PyRuntimeError::new_err`, `unwrap`/`expect`/`panic!`, and Python `RuntimeError`/`ValueError` raises).
- **Key findings:**
  - Session/bootstrap path still emits raw `PyRuntimeError` in `src/session.rs:26` and `src/session/bootstrap.rs:71,79,92`.
  - Runtime helpers rely on unverifiable panics/unwraps across `src/runtime/*` (see ISSUE-012 for enumerated locations including `frame_inspector.rs` and `value_encoder.rs`).
  - Monitoring plumbing uses `lock().unwrap()` extensively (`src/monitoring/tracer.rs:268-706`) and exposes `PyRuntimeError` directly.
  - Python facade raises built-in exceptions (`codetracer_python_recorder/session.py:34,57,62`).
- **Follow-up tracking:** New issue entries recorded in `issues.md` — ISSUE-011 (session/bootstrap), ISSUE-012 (runtime), ISSUE-013 (monitoring/FFI), ISSUE-014 (Python facade).
- **Next actions:** Socialise the audit command with the team; begin refactoring session/bootstrap sites under ISSUE-011 once ADR 0004 is accepted.

## Upcoming Workstreams
- WS2–WS8 remain **Not started** pending completion of WS1 groundwork and ADR acceptance.

