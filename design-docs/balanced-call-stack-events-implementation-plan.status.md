# Balanced Call Stack Events – Status

## Relevant Design Docs
- `design-docs/adr/0012-balanced-call-stack-events.md`
- `design-docs/balanced-call-stack-events-implementation-plan.md`
- `design-docs/design-001.md` (monitoring architecture reference)

## Key Source Files
- `codetracer-python-recorder/src/runtime/tracer/events.rs`
- `codetracer-python-recorder/src/monitoring/mod.rs`
- `codetracer-python-recorder/src/monitoring/install.rs`
- `codetracer-python-recorder/src/monitoring/callbacks.rs`
- `codetracer-python-recorder/src/runtime/tracer/runtime_tracer.rs`
- `codetracer-python-recorder/tests/python/test_monitoring_events.py`
- `codetracer-python-recorder/tests/rust/print_tracer.rs`

## Workstream Progress

### WS1 – Monitoring Mask & Callback Wiring
- **Scope recap:** Update `RuntimeTracer::interest` to include `PY_YIELD`, `PY_UNWIND`, `PY_RESUME`, and `PY_THROW`; ensure installer wiring respects the expanded mask; document the call/return mapping in `design-001`.
- **Status:** _Completed_
  - `RuntimeTracer::interest` now subscribes to the four additional events plus `LINE`.
  - `design-docs/design-001.md` documents the call vs return mapping and clarifies how each event is encoded.
  - Verification: `just test codetracer-python-recorder --all-targets` (passes).

### WS2 – Call/Return Edge Helpers
- **Status:** Not started (awaiting WS1 completion).

### WS3 – Activation & Lifecycle Behaviour
- **Status:** Not started.

### WS4 – Testing & Validation
- **Status:** Not started.

## Next Checkpoints
1. Modify `RuntimeTracer::interest` and related tests to assert the expanded mask.
2. Update `design-docs/design-001.md` to document the event-to-writer mapping.
3. Capture verification notes (e.g., `just test codetracer-python-recorder --all-targets`) once WS1 lands.
