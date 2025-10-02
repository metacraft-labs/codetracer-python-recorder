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
- ⏳ Step 2: Extend `ActivationController` with orchestration helpers and update callbacks accordingly.

## Next Actions
- Draft short notes on activation gating and frame search mechanics to complete Stage 0.
- Extend `ActivationController` API and callback usage (Stage 2 – Step 2).
