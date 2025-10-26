# Balanced Call Stack Events – Implementation Plan

Plan owners: codetracer recorder maintainers  
Target ADR: 0012 – Balanced sys.monitoring Call Stack Events  
Impacted components: `codetracer-python-recorder/src/monitoring`, `src/runtime/tracer`, `src/runtime/value_capture`, Python integration tests

## Goals
- Subscribe to `PY_UNWIND`, `PY_YIELD`, `PY_RESUME`, and `PY_THROW` so the recorder observes every frame transition that affects the logical call stack.
- Emit `TraceWriter::register_call` for resume/throw events and `TraceWriter::register_return` for yield/unwind events, keeping the on-disk trace strictly balanced without changing the format.
- Preserve existing policies (filters, activation gating, IO capture, error handling) for the newly supported events.
- Ship regression tests that prove generators, coroutines, and exception unwinds no longer leave dangling activations.

## Non-Goals
- No new trace record kinds or schema updates—the file format will continue to expose only call/return/line records.
- No STOP_ITERATION or async-step support in this iteration; we only handle the four events required for stack balancing.
- No new Python-facing APIs or CLI flags; behaviour changes remain internal to the recorder.

## Current Gaps
- `RuntimeTracer::interest` (`src/runtime/tracer/events.rs`) unions only `PY_START`, `LINE`, and `PY_RETURN`, so CPython never calls our callbacks for yield/resume/unwind/throw events.
- Callback implementations for `on_py_resume`, `on_py_yield`, `on_py_throw`, and `on_py_unwind` defer to the default trait methods, meaning no trace entries are emitted when those events fire.
- Activation gating (`ActivationController`) relies exclusively on `PY_RETURN` notifications to detect when the activation function exits and therefore never observes unwinds, while yields would prematurely end tracing if we naïvely treated them as "final" returns.
- Tests (`tests/python/test_monitoring_events.py`) only assert start/return pairs for direct calls, leaving generators and exception flows unverified.

## Workstreams

### WS1 – Monitoring Mask & Callback Wiring
**Scope:** Ensure the tracer actually receives the four additional events.
- Update `RuntimeTracer::interest` to include `PY_YIELD`, `PY_UNWIND`, `PY_RESUME`, and `PY_THROW` alongside the existing mask.
- Add documentation in `design-docs/design-001.md` detailing which `sys.monitoring` events map to `TraceWriter::register_call` vs `register_return`.
- Confirm `install_tracer` reuses the updated `EventSet`, and add assertions in the Rust unit tests that `interest` toggles the correct bits.
- Exit criteria: installing the tracer registers six Python events (start, resume, throw, return, yield, unwind) plus line.

### WS2 – Call/Return Edge Helpers
**Scope:** Share logic for emitting structural trace events across multiple callbacks.
- Introduce helper methods inside `RuntimeTracer` (e.g., `emit_call_edge(kind, code)` and `emit_return_edge(kind, code, payload)`) that encapsulate:
  - Activation gating (`should_process_event` + `should_trace_code`).
  - Filter lookups, telemetry handles, and IO flushes.
  - `TraceWriter::register_call`/`register_return` invocations, including empty argument lists for resume/throw, and descriptive labels (`"<yield>"`, `"<unwind>"`) for value redaction bookkeeping.
- Reuse existing `capture_call_arguments` only for `PY_START`. `PY_RESUME` will emit an empty argument vector because the callback does not expose the `send()` value, while `PY_THROW` should wrap the provided exception object into a synthetic argument named `exception` and encode it with the existing value encoder (no new types) so the resumed activation records what was injected (per <https://docs.python.org/3/library/sys.monitoring.html#monitoring-event-PY_THROW>).
- For `PY_THROW`, treat the incoming exception object as both (a) the return payload of the previous activation (because the generator exits up-stack) and (b) metadata recorded during the resumed activation? To keep semantics simple, treat `PY_THROW` strictly as a resume-side call edge with no immediate value capture.
- Ensure all return-like paths (`PY_RETURN`, `PY_UNWIND`, `PY_YIELD`) call `flush_pending_io()` before writing the record to keep streamed IO aligned with frame exits.
- Track event source in debug logs (`log_event`) so we can distinguish `on_py_return` vs `on_py_yield`, aiding support diagnostics.

### WS3 – Activation & Lifecycle Behaviour
**Scope:** Maintain activation gating correctness while adding new structural events.
- Extend `ActivationController` with a notion of "suspension" so `PY_YIELD` does **not** mark the activation as completed, while `PY_RETURN` and `PY_UNWIND` still shut it down. A simple approach is to thread an `ActivationExitKind` enum through `handle_return_event`.
- When a generator resumes (`PY_RESUME`/`PY_THROW`), ensure `should_process_event` continues to consult the activation state so suspended activations can keep tracing once re-entered but completed activations remain off.
- Confirm lifecycle flags (`mark_event`, `mark_failure`) are triggered for every emitted call/return and that `TraceWriter::finish_writing_trace_events` runs with a clean stack even when recording stops during an unwind.
- Update any telemetry counters or metadata summaries that assumed a 1:1 relationship between `PY_START` / `PY_RETURN`.

### WS4 – Testing & Validation
**Scope:** Prove generators, coroutines, and exceptions emit balanced traces.
- Python tests:
  - Add generator that yields twice and is resumed via `send()` and `throw()`. Assert the low-level trace (via `codetracer_python_recorder.trace`) contains matching call/return counts and that resumption after `throw()` still emits a call edge.
  - Add coroutine/async def test that awaits, ensuring `PY_YIELD` semantics also cover `await`.
  - Add exception unwinding test where a function raises; assert the trace file closes the activation (no dangling frames).
- Rust tests:
  - Enhance `tests/rust/print_tracer.rs` to count the new events when the integration feature is enabled, validating callback registration and error handling.
  - Unit-test the new helper functions with synthetic `CodeObjectWrapper` fixtures to ensure `emit_call_edge` emits empty argument vectors for resume events.
- Manual verification:
  - Run `just test` plus a targeted script that dumps the generated `.trace` file to check for alternating call/return records even when generators are partially consumed.
  - Capture performance samples on generator-heavy code to ensure callback overhead stays within existing thresholds (<10% regression).

## Testing & Rollout Checklist
- [ ] `cargo test` (workspace)  
- [ ] `just test` to exercise Python + Rust suites  
- [ ] Ensure new Python tests run under CPython 3.12 and 3.13 in CI  
- [ ] Validate that traces recorded from `examples/generator.py` (add if needed) now contain balanced call counts (e.g., via a small verification script)  
- [ ] Update release notes / changelog entry describing the new event coverage

## Risks & Mitigations
- **Double-emission of call/return pairs:** Mitigate via targeted tests that assert the writer stack depth never drops below zero and ends at zero after sample programs.
- **Activation path regressions:** `PY_YIELD` handling must not prematurely deactivate tracing; add regression tests that set `CODETRACER_ACTIVATION_PATH` to a generator and ensure tracing continues across resumes.
- **Unhandled payload types:** `PY_UNWIND` carries an exception object that might not be JSON serialisable when value capture is disabled. Guard `record_return_value` with the existing redaction policy and log errors before surfacing them to Python.
- **Performance overhead:** Monitor benchmarks once the feature lands. We expect only a handful of extra callbacks; if regression >10%, consider making resume/throw value capture lazier or batching events.
