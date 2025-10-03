# ADR 0002: Function-Level Single Responsibility Refactor

- **Status:** Proposed
- **Date:** 2025-10-15
- **Deciders:** Platform / Runtime Tracing Team
- **Consulted:** Python Tooling WG, Developer Experience WG

## Context

The codetracer runtime currently exposes several high-traffic functions that blend unrelated concerns, making them difficult to understand, test, and evolve.

- [`codetracer-python-recorder/src/session.rs:start_tracing`](../../codetracer-python-recorder/src/session.rs) performs logging setup, state guards, filesystem validation and creation, format parsing, Python metadata collection, tracer instantiation, and sys.monitoring installation within one 70+ line function.
- [`codetracer-python-recorder/src/runtime/mod.rs:on_py_start`](../../codetracer-python-recorder/src/runtime/mod.rs) handles activation gating, synthetic filename filtering, argument collection via unsafe PyFrame calls, error logging, and call registration in a single block.
- [`codetracer-python-recorder/src/runtime/mod.rs:on_line`](../../codetracer-python-recorder/src/runtime/mod.rs) interleaves activation checks, frame navigation, locals/globals materialisation, value encoding, variable registration, and memory hygiene for reference counted objects.
- [`codetracer-python-recorder/src/runtime/mod.rs:on_py_return`](../../codetracer-python-recorder/src/runtime/mod.rs) combines activation lifecycle management with value encoding and logging.
- [`codetracer-python-recorder/codetracer_python_recorder/session.py:start`](../../codetracer-python-recorder/codetracer_python_recorder/session.py) mixes backend state checks, path normalisation, format coercion, and PyO3 bridge calls.

These hotspots violate the Single Responsibility Principle at the function level. When we add new formats, richer activation flows, or additional capture types, we risk regressions because each modification touches fragile, monolithic code blocks.

## Decision

We will refactor high-traffic functions so that each public entry point coordinates narrowly-scoped helpers, each owning a single concern.

1. **Trace session start-up:** Introduce a `TraceSessionBootstrap` (Rust) that encapsulates directory preparation, format resolution, and program metadata gathering. `start_tracing` will delegate to helpers like `ensure_trace_directory`, `resolve_trace_format`, and `collect_program_metadata`. Python-side `start` will mirror this by delegating validation to dedicated helpers (`validate_trace_path`, `coerce_format`).
2. **Frame inspection & activation gating:** Extract frame traversal and activation decisions into dedicated helpers inside `runtime/frame_inspector.rs` and `runtime/activation.rs`. Callback methods (`on_py_start`, `on_line`, `on_py_return`) will orchestrate the helpers instead of performing raw pointer work inline.
3. **Value capture pipeline:** Move argument, locals, globals, and return value capture to a `runtime::value_capture` module that exposes high-level functions such as `capture_call_arguments(frame, code)` and `record_visible_scope(writer, frame)`. These helpers will own error handling and ensure reference counting invariants, allowing callbacks to focus on control flow.
4. **Logging and error reporting:** Concentrate logging into small, reusable functions (e.g., `log_trace_event(event_kind, code, lineno)`) so that callbacks do not perform ad hoc logging alongside functional work.
5. **Activation lifecycle:** Ensure `ActivationController` remains the single owner for activation state transitions. Callbacks will query `should_process_event` and `handle_deactivation` helpers instead of duplicating checks.

The refactor maintains public APIs but reorganises internal call graphs to keep each function focused on orchestration.

## Consequences

- **Positive:**
  - Smaller, intention-revealing functions improve readability and lower the mental load for reviewers modifying callback behaviour.
  - Reusable helpers unlock targeted unit tests (e.g., for path validation or locals capture) without invoking the entire tracing stack.
  - Error handling becomes consistent and auditable when concentrated in dedicated helpers.
  - Future features (streaming writers, selective variable capture) can extend isolated helpers rather than modifying monoliths.
- **Negative / Risks:**
  - Increased number of private helper modules/functions may introduce slight organisational overhead for newcomers.
  - Extracting FFI-heavy logic requires careful lifetime management; mistakes could introduce reference leaks or double-frees.
  - Interim refactors might temporarily duplicate logic until all call sites migrate to the new helpers.

## Implementation Guidelines

1. **Preserve semantics:** Validate each step with `just test` and targeted regression fixtures to ensure helper extraction does not change runtime behaviour.
2. **Guard unsafe code:** When moving PyFrame interactions, wrap unsafe blocks in documented helpers with clear preconditions and postconditions.
3. **Keep interfaces narrow:** Expose helper functions as `pub(crate)` or module-private to prevent leaking unstable APIs.
4. **Add focused tests:** Unit test helpers for error cases (e.g., invalid path, unsupported format, missing frame) and integrate them into existing test suites.
5. **Stage changes:** Land extractions in small PRs, updating the surrounding code incrementally to avoid giant rewrites.
6. **Document intent:** Update docstrings and module-level docs to describe helper responsibilities, keeping comments aligned with SRP boundaries.

## Alternatives Considered

- **Status quo:** Rejected; expanding functionality would keep bloating already-complex functions.
- **Entirely new tracer abstraction:** Unnecessary; existing `RuntimeTracer` shape is viable once responsibilities are modularised.

## Follow-Up

- Align sequencing with `design-docs/function-level-srp-refactor-plan.md`.
- Revisit performance benchmarks after extraction to ensure added indirection does not materially affect tracing overhead.
