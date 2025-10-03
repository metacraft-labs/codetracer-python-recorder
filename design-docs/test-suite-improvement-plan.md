# codetracer-python-recorder Test Suite Improvement Plan

## Goals
- Establish a transparent testing pyramid so engineers know whether new coverage belongs in Rust unit tests, Rust integration tests, or Python user-flow tests.
- Raise confidence in onboarding-critical paths (session bootstrap, activation gating, file outputs) by adding deterministic unit and integration tests.
- Reduce duplication and drift between Rust and Python harnesses by sharing fixtures and tooling.
- Prepare for future coverage metrics by making each harness runnable and observable in isolation.

## Current Pain Points
- Python-facing tests currently live in `codetracer-python-recorder/test/` while Rust integration tests live in `codetracer-python-recorder/tests/`; the near-identical names are easy to mis-type and confuse CI/job configuration.
- Core bootstrap logic lacks direct coverage: no existing test references `TraceSessionBootstrap::prepare` or helpers inside `codetracer-python-recorder/src/session/bootstrap.rs`, and `TraceOutputPaths::configure_writer` in `codetracer-python-recorder/src/runtime/output_paths.rs` is only exercised implicitly.
- `ActivationController` in `codetracer-python-recorder/src/runtime/activation.rs` is only touched indirectly through long integration scripts, leaving edge cases (synthetic filenames, multiple activation toggles) unverified.
- Python helpers `_coerce_format`, `_validate_trace_path`, and `_normalize_activation_path` in `codetracer-python-recorder/codetracer_python_recorder/session.py` are not unit tested; regressions would surface only during end-to-end runs.
- `just test` hides which harness failed because both `cargo nextest run` and `pytest` report together; failures require manual reproduction to determine the responsible layer.

## Workstreams

### WS1 – Layout Consolidation & Tooling Updates
- Rename Python test directory to `codetracer-python-recorder/tests/python/` and move existing files.
- Move Rust integration tests into `codetracer-python-recorder/tests/rust/` and update `Cargo.toml` (if necessary) to ensure cargo discovers them.
- Add `codetracer-python-recorder/tests/README.md` describing the taxonomy and quick-start commands.
- Update `Justfile`, `pyproject.toml`, and any workflow scripts to call `pytest tests/python` explicitly.
- Exit criteria: `just test` logs identify `cargo nextest` and `pytest tests/python` as separate steps, and developers can run each harness independently.

### WS2 – Rust Bootstrap Coverage
- Add `#[cfg(test)]` modules under `codetracer-python-recorder/src/session/bootstrap.rs` covering:
  - Directory creation success and failure (non-directory path, unwritable path).
  - Format resolution, including legacy aliases and error cases.
  - Program metadata capture when `sys.argv` is empty or contains non-string values.
- Add tests for `TraceOutputPaths::new` and `configure_writer` under `codetracer-python-recorder/src/runtime/output_paths.rs`, using an in-memory writer stub to assert emitted file names and initial start position.
- Exit criteria: failures in any helper produce precise `PyRuntimeError` messages, and the new tests fail if error handling regresses.

### WS3 – Activation Controller Guard Rails
- Introduce focused unit tests for `ActivationController` (e.g., `#[cfg(test)]` alongside `codetracer-python-recorder/src/runtime/activation.rs`) covering:
  - Activation path matching and non-matching filenames.
  - Synthetic filename rejection (`<string>` and `<stdin>`).
  - Multiple activation cycles to ensure `activation_done` prevents re-entry.
- Extend existing `RuntimeTracer` tests to add a regression asserting that disabling synthetic frames keeps `CallbackOutcome::DisableLocation` consistent.
- Exit criteria: Activation tests run without spinning up full integration scripts and cover both positive and negative flows.

### WS4 – Python API Unit Coverage & Fixtures
- Create a `tests/python/unit/` package with tests for `_coerce_format`, `_validate_trace_path`, `_normalize_activation_path`, and `TraceSession` life-cycle helpers (`flush`, `stop` when inactive).
- Extract reusable Python fixtures (temporary trace directory, environment manipulation) into `tests/python/support/` for reuse by integration tests.
- Confirm high-level tests (e.g., `test_monitoring_events.py`) import shared fixtures instead of duplicating temporary directory logic.
- Exit criteria: Python unit tests run without initialising the Rust extension, and integration tests rely on shared fixtures to minimise duplication.

### WS5 – CI & Observability Enhancements
- Update CI workflows to surface separate status checks (e.g., `rust-tests`, `python-tests`).
- Add minimal coverage instrumentation: enable `cargo llvm-cov` (or `grcov`) for Rust helpers and `pytest --cov` for Python tests, even if we only publish the reports as artefacts initially.
- Document required commands in `tests/README.md` and ensure `just test` forwards `--nocapture`/`-q` flags appropriately.
- Exit criteria: CI reports the two harnesses independently, and developers can opt-in to coverage locally following documented steps.

## Sequencing & Milestones

1. **Stage 0 – Baseline (1 PR)**
   - Capture current `just test` runtime and identify flaky tests.
   - Snapshot trace files produced by `tests/test_monitoring_events.py` for regression comparison.

2. **Stage 1 – Layout Consolidation (1–2 PRs)**
   - Execute WS1: rename directories, update tooling, land `tests/README.md`.

3. **Stage 2 – Bootstrap & Output Coverage (1 PR)**
   - Execute WS2; ensure new tests pass on Linux/macOS runners.

4. **Stage 3 – Activation Guard Rails (1 PR)**
   - Execute WS3; ensure synthetic filename handling remains guarded.

5. **Stage 4 – Python Unit Coverage (1 PR)**
   - Execute WS4; migrate existing integration tests to shared fixtures.

6. **Stage 5 – CI & Coverage Instrumentation (1 PR)**
   - Execute WS5; update workflow files and document developer commands.

7. **Stage 6 – Cleanup & Documentation (optional PR)**
   - Update ADR status to **Accepted**, refresh onboarding docs, and archive baseline trace fixtures.

## Verification Strategy
- Run `just test` after each stage; ensure both harnesses are explicitly reported in CI logs.
- Add `cargo nextest run --tests --nocapture activation::tests` smoke job to confirm activation unit coverage.
- For Python, run `pytest tests/python/unit -q` in isolation to keep the unit layer fast and deterministic.
- Compare stored trace fixtures before/after coverage additions to confirm no behavioural regressions.

## Risks & Mitigations
- **Path renames break imports:** mitigate by landing directory changes alongside import updates and running `pytest -q` locally before merge.
- **Increased test runtime:** unit tests are lightweight; integration tests already dominate runtime. Monitor `just test` duration and consider parallel pytest execution if needed.
- **Coverage tooling churn:** start with optional coverage reports to avoid blocking CI; formal thresholds can follow once noise is understood.
- **PyO3 version mismatches:** ensure new Rust tests use `Python::with_gil` and `Bound<'_, PyAny>` consistently to avoid UB when running under coverage tools.

## Deliverables & Ownership
- Primary owner: Runtime Tracing Team.
- Supporting reviewers: Python Tooling WG for Python fixtures and QA Automation Guild for CI changes.
- Target completion: end of Q4 FY25, ahead of planned streaming-writer work that depends on reliable regression coverage.

