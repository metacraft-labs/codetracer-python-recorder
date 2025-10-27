# Toplevel Exit & Trace Gating – Status

## Relevant Design Docs
- `design-docs/adr/0015-balanced-toplevel-lifecycle-and-trace-gating.md`
- `design-docs/toplevel-exit-and-trace-gating-implementation-plan.md`

## Key Source Files
- `codetracer-python-recorder/src/session.rs`
- `codetracer_python_recorder/session.py`
- `codetracer_python_recorder/cli.py`
- `codetracer-python-recorder/src/monitoring/install.rs`
- `codetracer-python-recorder/src/monitoring/mod.rs`
- `codetracer-python-recorder/tests/python` (planned additions)

## Workstream Progress

### WS1 – Public API & Session Plumbing
- **Scope recap:** Teach `stop_tracing` to accept an optional exit code and propagate it through the Python helpers and CLI while keeping backwards compatibility.
- **Status:** _Completed_
  - `stop_tracing` now accepts an optional `exit_code` argument, and the Python session helpers/CLI forward the value.
  - Added unit coverage ensuring `stop(exit_code=...)` forwards the value downstream while `stop()` remains valid.
  - Verification: `just dev` (editable build with `integration-test` feature) and `just py-test` (Python suites across both recorders) pass.

### WS2 – Runtime Exit State & `<toplevel>` Return Emission
- **Status:** _Not Started_ (blocked on WS1 API plumbing).

### WS3 – Unified Trace Gate Abstraction
- **Status:** _Not Started_.

### WS4 – Lifecycle & Metadata Updates
- **Status:** _Not Started_.

### WS5 – Validation & Parity Follow-Up
- **Status:** _Not Started_.

## Notes
- API changes will require a minor version bump once runtime support lands; capture release planning tasks after WS2.
- Remember to bootstrap the dev build (`just dev`) before Python suites so integration-test hooks stay enabled.
