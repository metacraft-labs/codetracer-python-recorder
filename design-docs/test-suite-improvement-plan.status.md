# Test Suite Improvement Plan Status

## Stage Summary
- ✅ Stage 0 – Baseline captured by ADR 0003 and the initial improvement plan.
- ✅ Stage 1 – Layout Consolidation: directory moves completed, test commands
  updated, README added, and `just test` now runs the Rust and Python harnesses
  separately.
- ⏳ Stage 2 – Bootstrap & Output Coverage: not started.
- ⏳ Stage 3 – Activation Guard Rails: not started.
- ⏳ Stage 4 – Python Unit Coverage: not started.
- ⏳ Stage 5 – CI & Coverage Instrumentation: not started.
- ⏳ Stage 6 – Cleanup & Documentation: not started.

## Next Actions
1. Prepare CI updates to split Rust and Python harness reporting so the new
   layout is visible in automation (falls under Stage 5).
2. Start Stage 2 by adding focused unit coverage for session bootstrap helpers.
