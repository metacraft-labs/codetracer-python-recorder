# Function-Level SRP Refactor Status

## Stage 0 – Baseline & Guardrails
- ✅ `just test` (Rust + Python suites) passes; captured run via the top-level recipe.
- ✅ Generated JSON and binary reference traces from `examples/value_capture_all.py`; outputs stored in `artifacts/stage0/value-capture-json/` and `artifacts/stage0/value-capture-binary/`.
- ⏳ Summarise current `ActivationController` behaviour and frame traversal notes for reviewer context.

## Stage 1 – Session Start-Up Decomposition
- ✅ Step 1 (Rust): Introduced `session::bootstrap` helpers and refactored `start_tracing` to delegate directory validation, format resolution, and program metadata collection. Tests remain green.
- ⏳ Step 2 (Python): Extract validation helpers in `codetracer_python_recorder/session.py`.

## Next Actions
- Draft short notes on activation gating and frame search mechanics to complete Stage 0.
- Extract Python-side validation helpers (Stage 1 – Step 2).
