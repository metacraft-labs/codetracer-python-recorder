# Function-Level Single Responsibility Refactor Plan

## Goals
- Ensure each public function in the tracer stack orchestrates a single concern, delegating specialised work to cohesive helpers.
- Reduce unsafe code surface inside high-level callbacks by centralising frame manipulation and activation logic.
- Improve testability by exposing narrow helper functions that can be unit tested without spinning up a full tracing session.

## Hotspot Summary
| Function | Location | Current mixed responsibilities |
| --- | --- | --- |
| `start_tracing` | `codetracer-python-recorder/src/session.rs` | Logging bootstrap, active-session guard, filesystem validation/creation, format parsing, argv inspection, tracer construction, sys.monitoring registration |
| `start` | `codetracer_python_recorder/session.py` | Backend state guard, path coercion, format normalisation, activation path handling, PyO3 call |
| `RuntimeTracer::on_py_start` | `codetracer-python-recorder/src/runtime/mod.rs` | Activation gating, synthetic filename filtering, unsafe frame acquisition, argument capture, writer registration, logging |
| `RuntimeTracer::on_line` | `codetracer-python-recorder/src/runtime/mod.rs` | Activation gating, frame search, locals/globals materialisation, value encoding, variable registration, logging |
| `RuntimeTracer::on_py_return` | `codetracer-python-recorder/src/runtime/mod.rs` | Activation gating, return value encoding, activation state transition, logging |

These functions currently exceed 60–120 lines and interleave control flow with low-level detail, making them brittle and difficult to extend.

## Refactor Strategy
1. **Codify shared helpers before rewriting call sites.** Introduce new modules (`runtime::frame_inspector`, `runtime::value_capture`, `session::bootstrap`) that encapsulate filesystem, activation, and frame-handling behaviour.
2. **Convert complex functions into orchestration shells.** After helpers exist, shrink the hotspot functions to roughly 10–25 lines that call the helpers and translate their results into tracer actions.
3. **Add regression tests around extracted helpers** so that future changes to callbacks can lean on focused coverage instead of broad integration tests.
4. **Maintain behavioural parity** by running full `just test` plus targeted fixture comparisons after each stage.

## Work Breakdown

### Stage 0 – Baseline & Guardrails (1 PR)
- Confirm the repository is green (`just test`).
- Capture representative trace output fixtures (binary + JSON) to compare after refactors.
- Document current behaviour of `ActivationController` and frame traversal in quick notes for reviewers.

### Stage 1 – Session Start-Up Decomposition (Rust + Python) (2 PRs)
1. **Rust bootstrap helper**
   - Add `session/bootstrap.rs` (or equivalent module) exposing functions `ensure_trace_directory`, `resolve_trace_format`, `collect_program_metadata`.
   - Refactor `start_tracing` to call these helpers; keep public signature unchanged.
   - Unit test each helper for error cases (invalid path, unsupported format, argv fallback).

2. **Python validation split**
   - Extract `validate_trace_path` and `coerce_format` into private helpers in `session.py`.
   - Update `start` to orchestrate helpers and call `_start_backend` only after validation succeeds.
   - Extend Python tests for duplicate start attempts and invalid path/format scenarios.

### Stage 2 – Frame Inspection & Activation Separation (Rust) (2 PRs)
1. **Frame locator module**
   - Introduce `runtime/frame_inspector.rs` handling frame acquisition, locals/globals materialisation, and reference-count hygiene.
   - Provide safe wrappers returning domain structs (e.g., `CapturedFrame { locals, globals, frame_ptr }`).
   - Update `on_line` to use the new inspector while retaining existing behaviour.

2. **Activation orchestration**
   - Enrich `ActivationController` with methods `should_process(code)` and `handle_deactivation(code_id)` so callbacks can early-return without duplicating logic.
   - Update `on_py_start`, `on_line`, and `on_py_return` to rely on these helpers.

### Stage 3 – Value Capture Layer (Rust) (2 PRs)
1. **Argument capture helper**
   - Create `runtime/value_capture.rs` (or expand existing module) exposing `capture_call_arguments(writer, frame, code)`.
   - Refactor `on_py_start` to use it, ensuring error propagation remains explicit.
   - Unit test for positional args, varargs, kwargs, non-string keys, and failure cases (e.g., failed locals sync).

2. **Scope recording helper**
   - Extract locals/globals iteration into `record_visible_scope(writer, captured_frame)`.
   - Update `on_line` to delegate the loop and remove inline Set bookkeeping.
   - Add tests covering overlapping names, `__builtins__` filtering, and locals==globals edge cases.

### Stage 4 – Return Handling & Logging Harmonisation (Rust) (1 PR)
- Introduce small logging helpers (e.g., `log_event(event, code, lineno)`).
- Provide `record_return_value(writer, value)` in `value_capture`.
- Refactor `on_py_return` to call activation decision, logging helper, and value recorder sequentially.
- Ensure deactivation on activation return remains tested.

### Stage 5 – Cleanup & Regression Sweep (1 PR)
- Remove obsolete inline comments / TODOs made redundant by helpers.
- Re-run `just test`, compare fixtures, and update docs referencing the old function shapes.
- Add final documentation pointing to the new helper modules for contributors.

## Testing Strategy
- **Unit tests:** Add Rust tests for each new helper module using PyO3 `Python::with_gil` harnesses and synthetic frames. Add Python tests for new validation helpers.
- **Integration tests:** Continue running `just test` after each stage. Augment with targeted scripts that exercise activation path, async functions, and nested frames to confirm instrumentation parity.
- **Fixture diffs:** Compare generated trace outputs (binary + JSON) before and after the refactor to ensure no semantic drift.

## Dependencies & Coordination
- Stage 1 must land before downstream stages to stabilise shared session APIs.
- Stages 2 and 3 can progress in parallel once bootstrap helpers are merged, but teams should sync on shared structs (e.g., `CapturedFrame`).
- Any changes to unsafe frame handling require review from at least one PyO3 domain expert.
- Update ADR 0002 status from “Proposed” to “Accepted” once Stages 1–4 merge successfully.

## Risks & Mitigations
- **Unsafe code mistakes:** Wrap raw pointer usage in RAII helpers with debug assertions; add fuzz/ stress tests for recursion-heavy scripts.
- **Performance regressions:** Benchmark tracer overhead before and after major stages; inline trivial helpers where necessary, or mark with `#[inline]` as appropriate.
- **Merge conflicts:** Finish each stage quickly and rebase branches frequently; keep PRs focused (≤400 LOC diff) to ease review.

