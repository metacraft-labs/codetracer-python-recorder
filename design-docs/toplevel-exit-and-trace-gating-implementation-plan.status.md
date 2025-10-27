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
- **Status:** _Completed_
  - `RuntimeTracer` now tracks a `SessionExitState`, emits the `<toplevel>` return during `finish`, and differentiates between explicit exit codes, default exits, and policy-driven disables.
  - Added trait plumbing (`Tracer::set_exit_status`) plus installer wiring so `stop_tracing` can forward the exit code before teardown.
  - Verification: `just cargo-test` (workspace) and `just py-test` exercises the new Rust test (`finish_emits_toplevel_return_with_exit_code`) and Python integration tests (`test_exit_payloads`).

### WS3 – Unified Trace Gate Abstraction
- **Status:** _Not Started_.

### WS4 – Lifecycle & Metadata Updates
- **Status:** _Not Started_.

### WS5 – Validation & Parity Follow-Up
- **Status:** _Not Started_.

## Notes
- API changes will require a minor version bump once runtime support lands; capture release planning tasks after WS2.
- Remember to bootstrap the dev build (`just dev`) before Python suites so integration-test hooks stay enabled.
