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
- **Status:** _Completed_
  - `ActivationController` now tracks a suspended state and exposes `handle_exit(code_id, ActivationExitKind)`, so `PY_YIELD` transitions into suspension without disabling the activation while `PY_RETURN`/`PY_UNWIND` mark completion.
  - Resume events clear suspension via `should_process_event`, ensuring activation gating stays engaged until the generator/coroutine finishes.
  - Added Rust unit tests covering the suspension/resume flow, and the runtime now routes return-edge handling through the new enum to keep lifecycle state consistent.
  - Verification: `just dev test` passes end-to-end.

### WS4 – Testing & Validation
- **Status:** Not started.

## Next Checkpoints
1. Expand WS4 coverage per plan (async awaits, throw/resume, unwind) and update rust/python integration tests accordingly.
2. Add rust-side assertions (e.g., `print_tracer`) to validate the expanded event mask.
3. Document any telemetry updates or metadata changes before shipping the feature.
