# ADR 0015: Balanced Toplevel Lifecycle and Unified Trace Gating

- **Status:** Proposed
- **Date:** 2025-03-21
- **Deciders:** codetracer recorder maintainers
- **Consulted:** DX tooling stakeholders, runtime tracing SMEs
- **Informed:** Support engineering, product analytics, replay consumers

## Context
- The recorder seeds every trace with a synthetic `<toplevel>` call when `TraceWriter::start` is invoked from `TraceOutputPaths::configure_writer` (`codetracer-python-recorder/src/runtime/output_paths.rs`). That call models the Python process entrypoint but the runtime never emits the matching return edge.
- CLI and API entrypoints (`codetracer_python_recorder/cli.py`, `codetracer_python_recorder/session.py`) already capture the script's exit status, yet the Rust runtime is oblivious to it, so the trace file looks like the script is still running when recording ends.
- Runtime gating currently combines two orthogonal systems: the legacy activation controller (`codetracer-python-recorder/src/runtime/activation.rs`) that defers tracing until a configured file executes, and the newer `TraceFilterEngine` (`codetracer-python-recorder/src/runtime/tracer/filtering.rs`) that offers scope-level allow/deny decisions. Both mechanisms decide whether an event should be written, but they execute independently and cache their own state.
- Because activation and filtering have separate caches and lifecycle hooks, downstream policies (value capture, IO flushes) see inconsistent state: a filter-suppressed frame still triggers activation bookkeeping, and activation suspensions do not propagate into the filter cache. The split also makes it hard to reason about which events will be recorded in the presence of chained filters.

## Problem
- **Unbalanced traces:** Without a `<toplevel>` return, consumers reconstructing the call stack see a dangling activation at depth 0. This breaks invariants in `TraceWriter::finish_writing_trace_events`, forces replay tools to special-case the synthetic frame, and hides the script's exit code even though callers already have it.
- **Duplicated gating logic:** Activation and filter decisions contradict one another in edge cases. For example:
  - When activation gates tracing until a file runs, the filter still caches a `TraceDecision::Trace` for the same code object, so subsequent resumes bypass activation because the filter short-circuits to "disable location".
  - When filters skip a frame, activation has no way to learn that the frame completed; its suspended/completed bookkeeping only triggers on return events that never fire for filter-disabled frames.
- The divergent implementations increase bug surface area (e.g., dangling activations, stale filter caches) and make it challenging to add new recorder policies that need a consistent view of "is this event observable?".

## Decision
1. **Emit a `<toplevel>` return carrying process exit status.**
   - Extend the PyO3 surface so `stop_tracing` accepts an optional integer exit code (default `None`). The Python helpers (`session.stop`, CLI `main`) will pass the script's final status.
   - Add a `TraceSessionGuard` helper on the Rust side that stores the provided exit status until `RuntimeTracer::finish` runs. When `finish` executes, it must:
     - Flush pending IO.
     - Record the exit status via `TraceWriter::register_return`, tagging the payload as `<exit>` when the code is unknown (e.g., interpreter crash) and serialising the integer otherwise.
     - Only then call the existing `finalise`/`cleanup` routines.
   - If tracing aborts early (`OnRecorderError::Disable` or fatal errors), emit a `<toplevel>` return with a synthetic reason (`"<disabled>"` / captured exception) so the stack always balances.
2. **Unify activation and filter decisions behind a shared gate.**
   - Introduce a `TraceGate` service managed by `RuntimeTracer`. The gate combines activation state and filter results into a single `GateDecision { process_event, disable_location, activation_event }`.
   - `FilterCoordinator` becomes responsible for caching scope resolutions only when `TraceGate` reports that the frame was actually processed. When the gate denies an event, both activation and filter caches are notified so they can mark the code id as "ignored" in lockstep.
   - `ActivationController` exposes explicit transitions (`on_enter`, `on_suspend`, `on_exit`) rather than letting callbacks poke its internal flags. `TraceGate` translates filter outcomes into the appropriate activation transition (e.g., a filter skip counts as `on_exit` so suspended activations resume correctly).
   - All tracer callbacks (`on_py_start`, `on_py_return`, `on_py_yield`, etc.) ask the gate for a decision before doing work. They honour `disable_location` uniformly, so CPython stops invoking us for code objects that either filter or activation wants to suppress.
   - Document the merged semantics: activation remains a coarse gate (enabling/disabling the root frame), filters apply fine-grained scope policies, and both share a single cache lifetime tied to tracer flush/reset.
3. **Update metadata and tooling expectations.**
   - Persist the recorded exit status into trace metadata (`trace_metadata.json`) so downstream tools can rely on it without scanning events.
   - Update docs and integration tests to assert that traces end at stack depth zero, even when activation suspends/resumes or filters drop frames.

## Consequences
- **Benefits**
  - Trace consumers no longer see dangling `<toplevel>` activations, and they can surface the script exit status directly from the trace file.
  - Activation, filtering, value capture, and IO policies share a single gating decision, reducing state divergence and simplifying future features (e.g., per-filter activation windows).
  - Error paths become easier to reason about because every exit funnels through the same `<toplevel>` return emission.
- **Costs**
  - API changes propagate through the Python bindings (`stop_tracing`, `TraceSession.stop`, CLI), requiring coordination with users embedding the recorder programmatically.
  - The gate abstraction adds code churn in `runtime/tracer/events.rs` and related helpers as callbacks adopt the new decision API.
  - Metadata writers must update to include the exit status.
- **Risks**
  - Forgetting to pass the exit status from bespoke integrations (custom Python entrypoints) would regress behaviour back to "unknown exit". We mitigate this with a backwards-compatible default (`None` translates to `<unknown>` exit) and clear release notes.
  - A buggy gate implementation could over-disable callbacks, suppressing legitimate trace data. We will add regression tests covering activation+filter combinations (activation path inside a skipped scope, resumed generators, etc.) before rollout.
  - The PyO3 signature change may break ABI expectations if not versioned carefully. We will bump the crate minor version and document the new keyword argument.

## Alternatives
- **Emit the `<toplevel>` return entirely on the Python side.** Rejected because it would duplicate writer logic in Python, bypass IO flush/value capture, and fail when users call the PyO3 API directly from Rust.
- **Keep activation and filter gating separate but document the quirks.** Rejected: we already hit real bugs (unbalanced traces, stale caches), and layering more documentation will not solve the underlying inconsistency.
- **Deprecate activation now that filters exist.** Rejected because activation provides a simple UX for "start tracing when my script begins", which filters alone cannot replace without writing bespoke configs.

## References
- `codetracer-python-recorder/src/runtime/output_paths.rs`
- `codetracer-python-recorder/src/runtime/tracer/events.rs`
- `codetracer-python-recorder/src/runtime/activation.rs`
- `codetracer-python-recorder/src/runtime/tracer/filtering.rs`
- `codetracer_python_recorder/session.py`
- `codetracer_python_recorder/cli.py`
