# Test Suite Improvement Plan (Short Form)

## Goals
- Clarify where tests belong (Rust unit vs. Rust integration vs. Python).
- Add coverage for session bootstrap, activation gating, and Python helpers.
- Keep each harness runnable on its own.

## Pain today
- Python tests live in `test/` and clash with Rust `tests/` naming.
- Bootstrap helpers and activation controller lack direct tests.
- Python helpers go untested; regressions surface only via slow end-to-end runs.
- `just test` prints combined output, so it’s hard to see which layer failed.

## Plan by stage
1. **Layout cleanup** – Move Python tests to `tests/python/`, Rust integration tests to `tests/rust/`, add `tests/README.md`, update tooling.
2. **Bootstrap coverage** – Unit-test `session::bootstrap` helpers and `runtime::output_paths` writer setup.
3. **Activation guard rails** – Unit-test `ActivationController` and confirm synthetic filenames return `DisableLocation`.
4. **Python unit layer** – Add `tests/python/unit` for `_coerce_format`, `_validate_trace_path`, `_normalize_activation_path`, etc., with shared fixtures in `tests/python/support`.
5. **CI polish** – Split CI jobs (`rust-tests`, `python-tests`), add optional coverage reports, and document commands in `tests/README.md`.
6. **Cleanup** – Update ADR status/docs and snapshot trace fixtures if needed.

## Verification
- Run `just test` plus the individual harness commands after each stage.
- Keep small unit suites fast (`pytest tests/python/unit -q`, targeted `cargo nextest` invocations).
- Compare stored trace fixtures before/after major changes.

## Risks
- Rename churn → land directory moves with matching import fixes.
- Runtime growth → monitor `just test`; consider parallel pytest if needed.
- Coverage noise → keep coverage optional until numbers stabilise.
