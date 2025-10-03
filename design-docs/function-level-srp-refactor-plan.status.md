# Function-Level SRP Refactor Status

## Stage 0 – Baseline & Guardrails
- ✅ `just test` (Rust + Python suites) passes; captured run via the top-level recipe.
- ✅ Generated JSON and binary reference traces from `examples/value_capture_all.py`; outputs stored in `artifacts/stage0/value-capture-json/` and `artifacts/stage0/value-capture-binary/`.
- ⏳ Summarise current `ActivationController` behaviour and frame traversal notes for reviewer context.

## Stage 1 – Session Start-Up Decomposition
- ✅ Step 1 (Rust): Introduced `session::bootstrap` helpers and refactored `start_tracing` to delegate directory validation, format resolution, and program metadata collection. Tests remain green.
- ✅ Step 2 (Python): Extracted `_coerce_format`, `_validate_trace_path`, and `_normalize_activation_path` helpers; added tests covering invalid formats and conflicting paths.

## Stage 2 – Frame Inspection & Activation Separation
- ✅ Step 1: Added `runtime::frame_inspector::capture_frame` to encapsulate frame lookup, locals/globals materialisation, and reference counting; `on_line` now delegates to the helper while preserving behaviour.
- ✅ Step 2: Extended `ActivationController` with `should_process_event`/`handle_return_event`, updated callbacks to rely on them, and removed direct state juggling from `RuntimeTracer`.

## Stage 3 – Value Capture Layer
- ✅ Step 1: Introduced `runtime::value_capture::capture_call_arguments`; `on_py_start` now delegates to it, keeping the function focused on orchestration while reusing frame inspectors.
- ✅ Step 2: Added `record_visible_scope` helper and refactored `on_line` to delegate locals/globals registration through it.

## Stage 4 – Return Handling & Logging Harmonisation
- ✅ Added `runtime::logging::log_event` to consolidate callback logging across start, line, and return handlers.
- ✅ Exposed `record_return_value` in `runtime::value_capture` and refactored `RuntimeTracer::on_py_return` to orchestrate activation checks, logging, and value recording.
- ✅ Extended runtime tests with explicit return capture coverage and activation deactivation assertions.

## Stage 5 – Cleanup & Regression Sweep
- ✅ Audited runtime modules for obsolete inline comments or TODOs introduced pre-refactor; none remained after helper extraction.
- ✅ Documented the helper module map in `design-docs/function-level-srp-refactor-plan.md` for contributor onboarding.
- ✅ Re-ran `just test` (Rust `cargo nextest` + Python `pytest`) to confirm post-cleanup parity.

## Next Actions
- Draft short notes on activation gating and frame search mechanics to complete Stage 0.
- Track Stage 5 fixture comparisons if we decide to snapshot JSON/Binary outputs post-refactor.
