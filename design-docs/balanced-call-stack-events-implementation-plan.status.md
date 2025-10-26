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
- **Status:** _Completed_
  - Added `RuntimeTracer::register_call_record` and `handle_return_edge` helpers so `PY_START`, `PY_RESUME`, `PY_THROW`, `PY_RETURN`, `PY_YIELD`, and `PY_UNWIND` share the same activation gating, filter, telemetry, and writer plumbing.
  - `PY_RESUME` now emits call edges with empty argument vectors, `PY_THROW` records an `exception` argument encoded via the existing value encoder, and `PY_YIELD`/`PY_UNWIND` reuse the return helper (no disable sentinel for unwind).
  - Python monitoring tests gained generator/yield/throw coverage to assert balanced trace output.
  - Verification: `just dev test` (maturin develop + cargo nextest + pytest) passes.

### WS3 – Activation & Lifecycle Behaviour
- **Status:** Not started.

### WS4 – Testing & Validation
- **Status:** Not started.

## Next Checkpoints
1. Begin WS3 by teaching `ActivationController` about suspension/resume semantics.
2. Plan and implement lifecycle tests ensuring activation gating stays consistent across yields/unwinds.
3. Evaluate whether additional telemetry/logging is needed before landing WS3/WS4.
