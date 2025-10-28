# Toplevel Exit & Trace Gating – Implementation Plan

Plan owners: codetracer recorder maintainers  
Target ADR: 0015 – Balanced Toplevel Lifecycle and Unified Trace Gating  
Impacted components:  
- `codetracer-python-recorder/src/session.rs` and `codetracer_python_recorder/session.py`  
- `codetracer-python-recorder/src/runtime/tracer` (events, lifecycle, filtering)  
- `codetracer-python-recorder/src/runtime/activation.rs`  
- `codetracer-python-recorder/src/runtime/output_paths.rs` and metadata helpers  
- `codetracer-pure-python-recorder` parity shims (optional but strongly recommended)

## Goals
- Always emit a `<toplevel>` return event whose payload reflects the process exit status (or a descriptive placeholder when unavailable).
- Plumb exit codes from Python entrypoints through the PyO3 API into the Rust runtime without breaking existing integrations.
- Replace the ad-hoc combination of activation and filter decisions with a single gate so callbacks make consistent trace/skip/disable choices.
- Keep lifecycle bookkeeping (IO flush, value capture, activation teardown) in sync with the unified gate and the new exit record.
- Extend metadata (`trace_metadata.json`) with the recorded exit status for downstream tooling.

## Non-Goals
- No changes to the on-disk trace schema beyond the new return record payload; we keep the existing call/return/line structure.
- No removal of activation support; the work only refactors it to cooperate with filters.
- No immediate addition of exit-status reporting to the CLI JSON trailers (can be follow-up).
- No attempt to refit the pure-Python recorder in the same PR; it may gain parity later but does not block landing the Rust changes.

## Current Gaps
- `stop_tracing` (PyO3) accepts no arguments, so the runtime never learns the script exit status captured by `codetracer_python_recorder/cli.py`.
- `RuntimeTracer::finish` only finalises writers; it does not record any return edge for the synthetic `<toplevel>` call emitted in `TraceWriter::start`.
- Activation and filtering are checked independently inside each callback (`on_py_start`, `on_py_return`, etc.), leading to divergent cache state (`ActivationController::suspended` vs `FilterCoordinator::ignored_code_ids`).
- Filter-driven `CallbackOutcome::DisableLocation` does not inform the activation controller, so the activation window can remain "active" after CPython stops issuing callbacks.
- Metadata writers do not persist exit status, and tests assume partial stacks are acceptable.

## Workstreams

### WS1 – Public API & Session Plumbing
**Scope:** Carry exit status from Python to Rust with backwards-compatible defaults.
- Update `codetracer-python-recorder/src/session.rs::stop_tracing` to expose an optional `exit_code: Option<i32>` parameter (new keyword-only arg in Python).
- Adjust `codetracer_python_recorder/session.py` so `TraceSession.stop` and the module-level `stop()` accept an optional exit code and forward it.
- Modify `codetracer_python_recorder/cli.py::main` to pass the captured `exit_code` when stopping the session; preserve legacy behaviour (`None`) for callers that do not provide a code.
- Add unit tests in `codetracer_python_recorder/tests` ensuring the new keyword argument is optional and that `stop(exit_code=123)` calls into the backend with the expected value (mocking PyO3 layer).

### WS2 – Runtime Exit State & `<toplevel>` Return Emission
**Scope:** Store the exit status and emit the balancing return event.
- Introduce a small struct (e.g., `SessionTermination`) inside `RuntimeTracer` to hold `exit_code: Option<i32>` plus a `reason: ExitReason`.
- Extend the `Tracer` trait implementation with a new method (e.g., `set_exit_status`) or reuse `notify_failure` paths to capture both normal exit and disable scenarios.
- In `RuntimeTracer::finish`, before `finalise`:
  - Call `record_return_value` with the exit payload.
  - Invoke `TraceWriter::register_return` / `mark_event`.
  - Ensure activation receives an `ActivationExitKind::Completed` for the toplevel code id.
- Emit synthetic reasons (`"<disabled>"`, `"<panic>"`, etc.) when tracing stops due to errors. Reuse existing error-path metadata in `notify_failure` to populate the reason.
- Update integration tests (Rust + Python) to assert that the final event sequence includes a `<toplevel>` call + return pair and that the return payload matches the script exit code.

### WS3 – Unified Trace Gate Abstraction
**Scope:** Merge activation and filter decision paths.
- Create `TraceGate` and `GateDecision` types under `runtime/tracer`.
  - API: `evaluate(py, code, event_kind) -> GateDecision`.
  - Decision carries: `process_event` (bool), `disable_location` (bool), `activation_transition` (enum).
- Refactor `FilterCoordinator` so it exposes `decide_for_gate(py, code)` returning an enriched result (trace/skip plus cached scope resolution). The coordinator only updates caches when the gate confirms the event was processed.
- Update `ActivationController` with methods `on_enter`, `on_suspend`, `on_exit`, and `reset`. Remove direct field mutations from callbacks.
- Rewrite tracer callbacks (`on_py_start`, `on_py_return`, `on_py_resume`, etc.) to:
  1. Ask the gate for a decision.
  2. Exit early when `process_event == false` and return the proper `CallbackOutcome`.
  3. After recording the event, invoke any activation transition included in the decision.
- Ensure `CallbackOutcome::DisableLocation` is only returned once per code id and that both activation and filter caches mark the frame as ignored thereafter.
- Unit test the gate with synthetic `CodeObjectWrapper` fixtures covering combinations: inactive activation + allow filter, active activation + skip filter, suspended activation resumed, etc.

### WS4 – Lifecycle & Metadata Updates
**Scope:** Keep writer lifecycle and metadata consistent with the new behaviour.
- Update `LifecycleController::finalise` (or adjacent helper) to write the exit status into `trace_metadata.json` under a new field (e.g., `"process_exit_code"`). Ensure this runs only once per session.
- Confirm `cleanup_partial_outputs` still executes when tracing disables early and that the exit record is only written for successful sessions.
- Add a regression test around `TraceOutputPaths::configure_writer` + `RuntimeTracer::finish` verifying the events buffer contains both call and return entries for `<toplevel>`.
- Update documentation (`design-docs/design-001.md`, user docs) to describe the exit-status metadata and the unified gating semantics.

### WS5 – Validation & Parity Follow-Up
**Scope:** Prove end-to-end correctness and plan the optional pure-Python update.
- Extend Python integration tests (`tests/python`) with scenarios:
  - CLI run that exits with non-zero status; assert trace contains `<toplevel>` return with the negative path.
  - Activation path configured alongside a filter that skips the same file; ensure tracing starts/stops exactly once and stack depth ends at zero.
  - Generator/coroutine workloads guarded by activation; confirm gate decisions do not regress existing balanced-call tests.
- Run `just test` to cover all tests after refactors.
- Document a follow-up issue to mirror `<toplevel>` return emission in `codetracer-pure-python-recorder`, keeping trace semantics aligned across products.

## Testing & Rollout Checklist
- [ ] `just test`
- [ ] Python integration tests covering exit-code propagation and activation+filter combinations
- [ ] Manual smoke test: run CLI against a script returning exit code 3, inspect `trace.json` for `<toplevel>` return payload `3`
- [ ] Update changelog / release notes highlighting the new API parameter and exit-status metadata
- [ ] Notify downstream data pipeline owners that exit status is now available

## Risks & Mitigations
- **Breaking API changes:** Ensure `stop_tracing` still works without arguments by providing a Python default (`exit_code: int | None = None`) and by releasing under a minor version bump.
- **Gate regressions:** Add exhaustive unit tests plus targeted integration tests so we catch scenarios where activation or filters no longer fire.
- **Performance impact:** Benchmark tracing hot paths after the refactor; the gate should add minimal overhead. Profile with `just bench` / existing benchmarks, and roll back micro-optimisations if regressions exceed 5%.
- **Incomplete error coverage:** Make sure disable/error paths still flush IO and write metadata. Write explicit tests that trigger `OnRecorderError::Disable` to observe the synthetic `<toplevel>` return reason.
