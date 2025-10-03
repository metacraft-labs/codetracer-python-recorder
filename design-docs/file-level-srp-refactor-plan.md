# File-Level Single Responsibility Refactor Plan

## Goals
- Reshape the Rust crate and Python support package so that every source file encapsulates a single cohesive topic.
- Reduce the amount of ad-hoc cross-module knowledge currently required to understand tracing start-up, event handling, and encoding logic.
- Preserve the public Python API and Rust crate interfaces during the refactor to avoid disruptions for downstream tooling.

## Current State Observations
- `src/lib.rs` is responsible for PyO3 module registration, lifecycle management for tracing sessions, global logging initialisation, and runtime format selection, which mixes unrelated concerns in one file.
- `src/runtime_tracer.rs` couples trace lifecycle control, activation toggling, and Python value encoding in a single module, making it difficult to unit test or substitute individual pieces.
- `src/tracer.rs` combines the `Tracer` trait definition, sys.monitoring shims, callback registration utilities, and thread-safe storage, meaning small changes can ripple through unrelated logic.
- `codetracer_python_recorder/api.py` interleaves environment based auto-start, context-manager ergonomics, backend state management, and format constants, leaving no clearly isolated entry-point for CLI or library callers.

## Target Rust Module Layout
| Topic | Target file | Notes |
| --- | --- | --- |
| PyO3 module definition & re-exports | `src/lib.rs` | Limit to module wiring plus `pub use` statements.
| Global logging defaults | `src/logging.rs` | Provide helper to configure env_logger defaults reused by both lib.rs and tests.
| Tracing session lifecycle (`start_tracing`, `stop_tracing`, `flush_tracing`, `is_tracing`) | `src/session.rs` | Own global `ACTIVE` flag and filesystem validation.
| Runtime tracer orchestration (activation gating, writer plumbing) | `src/runtime/mod.rs` | Public `RuntimeTracer` facade constructed by session.
| Value encoding helpers | `src/runtime/value_encoder.rs` | Convert Python objects into `runtime_tracing::ValueRecord` values; unit test in isolation.
| Activation management (start-on-enter logic) | `src/runtime/activation.rs` | Encapsulate `activation_path`, `activation_code_id`, and toggling state.
| Writer initialisation and file path selection | `src/runtime/output_paths.rs` | Determine file names for JSON/Binary and wrap TraceWriter begin/finish.
| sys.monitoring integration utilities | `src/monitoring/mod.rs` | Provide `ToolId`, `EventId`, `MonitoringEvents`, `set_events`, etc.
| Tracer trait & callback dispatch | `src/monitoring/tracer.rs` | Define `Tracer` trait and per-event callbacks; depend on `monitoring::events`.
| Code object caching | `src/code_object.rs` | Remains focused on caching; consider relocating question comments to doc tests.

The `runtime` and `monitoring` modules become directories with focused submodules, while `session.rs` consumes them via narrow interfaces. Any PyO3 FFI helper functions should live close to their domain (e.g., frame locals helpers inside `runtime/mod.rs`).

## Target Python Package Layout
| Topic | Target file | Notes |
| --- | --- | --- |
| Public API surface (`start`, `stop`, `is_tracing`, constants) | `codetracer_python_recorder/api.py` | Keep the public signatures unchanged; delegate to new helpers.
| Session handle implementation | `codetracer_python_recorder/session.py` | Own `TraceSession` class and backend delegation logic.
| Auto-start via environment variables | `codetracer_python_recorder/auto_start.py` | Move `_auto_start_from_env` and constants needed only for boot-time configuration.
| Format constants & validation | `codetracer_python_recorder/formats.py` | Define `TRACE_BINARY`, `TRACE_JSON`, `DEFAULT_FORMAT`, and any helpers to negotiate format strings.
| Module-level `__init__` exports | `codetracer_python_recorder/__init__.py` | Re-export the API and trigger optional auto-start.

Splitting the Python helper package along these lines isolates side-effectful auto-start logic from the plain API and simplifies targeted testing.

## Implementation Roadmap

1. **Stabilise tests and build scripts**
   - Ensure `just test` passes to establish a green baseline.
   - Capture benchmarks or representative trace outputs to validate parity later.

2. **Introduce foundational Rust modules (serial)**
   - Extract logging initialisation into `logging.rs` and update `lib.rs` to call the helper.
   - Move session lifecycle logic from `lib.rs` into a new `session.rs`, keeping function signatures untouched and re-exporting via `lib.rs`.
   - Update module declarations and adjust imports; verify tests.

3. **Restructure runtime tracer internals (can parallelise subtasks)**
   - Create `src/runtime/mod.rs` as façade exposing `RuntimeTracer`.
   - **Task 3A (Team A)**: Extract activation control into `runtime/activation.rs`, exposing a small struct consumed by the tracer.
   - **Task 3B (Team B)**: Extract value encoding routines into `runtime/value_encoder.rs`, providing unit tests and benchmarks.
   - **Task 3C (Team C)**: Introduce `runtime/output_paths.rs` to encapsulate format-to-filename mapping and writer initialisation.
   - Integrate submodules back into `runtime/mod.rs` sequentially once individual tasks are complete; resolve merge conflicts around struct fields.

4. **Modularise sys.monitoring glue (partially parallel)**
   - Add `monitoring/mod.rs` hosting shared types (`EventId`, `EventSet`, `ToolId`).
   - Split trait and dispatcher logic into `monitoring/tracer.rs`; keep callback registration helpers near the sys.monitoring bindings.
   - **Task 4A (Team A)**: Port OnceLock caches and registration helpers.
   - **Task 4B (Team B)**: Move `Tracer` trait definition and default implementations, updating call sites in runtime tracer and tests.

5. **Python package decomposition (parallel with Step 4 once Step 2 is merged)**
   - Create `session.py`, `formats.py`, and `auto_start.py` with extracted logic.
   - Update `api.py` to delegate to the new modules but maintain backward-compatible imports.
   - Adjust `__init__.py` to import from `api` and trigger optional auto-start via the new helper.
   - Update Python tests and examples to use the reorganised structure.

6. **Clean-up and follow-up tasks**
   - Remove obsolete comments (e.g., `//TODO AI!` placeholders) or move them into GitHub issues.
   - Update documentation and diagrams to reflect the new module tree.
   - Re-run `just test` and linting for both Rust and Python components; capture trace artifacts to confirm unchanged output format.

## Parallelisation Notes
- Step 2 touches the global entry points and should complete before deeper refactors to minimise rebasing pain.
- Step 3 subtasks (activation, value encoding, output paths) operate on distinct sections of the existing `RuntimeTracer`; they can be implemented in parallel once `runtime/mod.rs` scaffolding exists.
- Step 4's subtasks can proceed concurrently with Step 3 once the new `monitoring` module is introduced; teams should coordinate on shared types but work on separate files.
- Step 5 (Python package) depends on Step 2 so that backend entry-points remain stable; it can overlap with late Step 3/4 work because it touches only the Python tree.
- Documentation updates and clean-up in Step 6 can be distributed among contributors after core refactors merge.

## Testing & Verification Strategy
- Maintain existing integration and unit tests; add focused tests for newly separated modules (e.g., pure Rust tests for `value_encoder` conversions).
- Extend Python tests to cover environment auto-start logic now that it lives in its own module.
- For each phase, compare generated trace files against baseline fixtures to guarantee no behavioural regressions.
- Require code review sign-off from domain owners for each phase to ensure the single-responsibility intent is preserved.
