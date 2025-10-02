# Test Suite Improvement Plan Status

## Stage Summary
- ✅ Stage 0 – Baseline captured by ADR 0003 and the initial improvement plan.
- ✅ Stage 1 – Layout Consolidation: directory moves completed, test commands
  updated, README added, and `just test` now runs the Rust and Python harnesses
  separately.
- ✅ Stage 2 – Bootstrap & Output Coverage: unit tests now exercise
  `TraceSessionBootstrap` helpers (directory/format/argv handling) and
  `TraceOutputPaths::configure_writer`, with `just test` covering the new cases.
- ✅ Stage 3 – Activation Guard Rails: added unit tests around
  `ActivationController` covering activation start, non-matching frames, and
  deactivation behaviour; existing runtime integration tests continue to pass.
- ⏳ Stage 4 – Python Unit Coverage: not started.
- ⏳ Stage 5 – CI & Coverage Instrumentation: not started.
- ⏳ Stage 6 – Cleanup & Documentation: not started.

## Next Actions
1. Advance Stage 4 by creating Python unit tests for facade helpers and shared
   fixtures.
2. Plan Stage 5 CI split once the new unit coverage lands.
