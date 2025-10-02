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
- ✅ Stage 4 – Python Unit Coverage: added `tests/python/unit/test_session_helpers.py`
  for facade utilities and introduced `tests/python/support` for shared
  fixtures; updated monitoring tests to use the helper directory builder.
- 🚧 Stage 5 – CI & Coverage Instrumentation: CI now runs Rust and Python
  suites in separate steps; coverage instrumentation plan still pending.
- ⏳ Stage 6 – Cleanup & Documentation: not started.

## Next Actions
1. Implement coverage collection (Just targets + CI artefacts) following the
   drafted plan while keeping steps non-blocking initially.
2. Evaluate report stability and prepare enforcement thresholds before Stage 6.
