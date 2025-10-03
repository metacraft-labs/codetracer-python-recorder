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
- ✅ Stage 5 – CI & Coverage Instrumentation: CI now runs the split Rust/Python
  test jobs plus a non-blocking coverage job that reuses `just coverage`, uploads
  LCOV/XML/JSON artefacts, and posts a per-PR summary comment.
- ✅ Stage 6 – Cleanup & Documentation: ADR 0003 is now Accepted, top-level
  docs describe the testing/coverage workflow, and the tests README references
  the CI coverage comment for contributors.

## Next Actions
Plan complete; monitor coverage baselines and propose enforcement thresholds in
a follow-up task.
