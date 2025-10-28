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
- **Status:** _Completed_
  - Added Python integration tests covering generator yield/resume sequences, `g.throw(...)` exception injection, coroutine awaits (`asyncio.run`), and plain exception unwinds to verify balanced call/return pairs and recorded payloads.
  - Added `test_coroutine_send_and_throw_events_capture_resume_and_exception` to exercise coroutine `send()` and `throw()` paths, asserting the additional call edges plus the encoded `exception` argument and final return payloads.
  - Extended `tests/rust/print_tracer.rs` with a focused scenario (`tracer_counts_resume_throw_and_unwind_events`) to prove that `PY_RESUME`, `PY_THROW`, `PY_YIELD`, and `PY_UNWIND` fire the expected number of times for a simple generator/unwind script.
  - Verification: `just dev test` (maturin develop + cargo nextest + pytest) now passes end-to-end.

## Next Checkpoints
1. Monitor nightly runs for regressions around generator/coroutine call balancing and expand coverage again if new CPython events appear.
2. Document any telemetry/logging updates before shipping the feature.
3. Prepare release notes / changelog entries summarising the balanced call-stack support once release packaging starts.
