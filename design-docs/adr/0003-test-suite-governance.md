# ADR 0003: Test Suite Governance for codetracer-python-recorder

- **Status:** Accepted
- **Date:** 2025-10-02
- **Deciders:** Platform / Runtime Tracing Team
- **Consulted:** Python Tooling WG, Developer Experience WG
- **Informed:** Reliability Engineering Guild

## Context

`codetracer-python-recorder` currently depends on three distinct harnesses: Rust unit tests inside the crate, Rust integration tests under `codetracer-python-recorder/tests/`, and Python tests under `codetracer-python-recorder/test/`. `just test` wires these together via `cargo nextest run` and `pytest`, but we do not document which behaviours belong to each layer. As a result:
- Contributors duplicate coverage (e.g., API happy paths exist both in Rust integration tests and Python tests) while other areas are untested (no references to `TraceSessionBootstrap::prepare`, `ensure_trace_directory`, or `TraceOutputPaths::configure_writer`).
- The `test/` vs `tests/` split is opaque to new maintainers and tooling; several CI linters only recurse into `tests/`, so Python-only changes can silently reduce coverage.
- Developers add integration-style assertions to Python tests that require spawning interpreters, even when the logic could be exercised cheaply in Rust.
- Doc examples risk drifting from executable reality because doctests are disabled to avoid invoking the CPython runtime.

Without a clear taxonomy for Rust vs. Python coverage, the test surface is growing unevenly and critical bootstrap/activation code remains unverified.

## Decision

We will adopt a tiered test governance model and reorganise the repository to make the boundaries explicit.

1. **Define a test taxonomy.**
   - `src/**/*.rs` unit tests (behind `#[cfg(test)]`) cover pure-Rust helpers, pointer/FFI safety shims, and error handling that does not need to cross the FFI boundary.
   - `codetracer-python-recorder/tests/rust/**` integration tests exercise PyO3 + CPython interactions (e.g., `CodeObjectRegistry`, `RuntimeTracer` callbacks) and may spin up embedded interpreters.
   - `codetracer-python-recorder/tests/python/**` houses all Python-driven tests (pytest/unittest) for public APIs, end-to-end tracing flows, and environment bootstrapping.
   - Documentation examples use doctests only when they can run without Python (otherwise they move into the appropriate test layer).

2. **Restructure the repository.**
   - Rename the existing Python `test/` directory to `tests/python/` and update tooling (`pytest` discovery, `pyproject.toml`, `Justfile`) accordingly.
   - Move Rust integration tests into `tests/rust/` (keeping module names unchanged) to mirror the taxonomy.
   - Introduce a `tests/README.md` that summarises the policy for future contributors.

3. **Codify placement rules.**
   - Every new test must state its target layer in the PR description and follow the directory conventions above.
   - Changes touching PyO3 shims (`session`, `runtime`, `monitoring`) must include at least one Rust test; changes to the Python facade (`codetracer_python_recorder`) must include Python coverage unless the change is rust-only plumbing.
   - Shared fixtures (temporary trace directories, sample scripts) live under `tests/support/` and are imported from both Rust and Python harnesses to avoid drift.

4. **Fill immediate coverage gaps.**
   - Add focused Rust unit tests for `TraceSessionBootstrap::prepare`, `ensure_trace_directory`, `resolve_trace_format`, and `collect_program_metadata`, including error paths (non-directory target, unsupported format, missing `sys.argv`).
   - Add unit tests for `TraceOutputPaths::new` and `configure_writer` to ensure the writer initialises metadata/events files and starts at the expected location.
   - Add deterministic tests for `ActivationController` covering activation on enter, deactivation on return, and behaviour when frames originate from synthetic filenames.
   - Extend Python tests to cover `_normalize_activation_path` and failure modes of `_coerce_format`/`_validate_trace_path` without booting the Rust tracer.

5. **Establish guardrails.**
   - Update CI to run `cargo nextest run --workspace --tests` and `pytest tests/python` explicitly, making the split visible in logs.
   - Track per-layer test counts in `tests/README.md` and flag regressions in coverage reports once we integrate coverage tooling.

## Consequences

- **Positive:**
  - Onboarding improves because contributors follow a documented decision tree when adding tests.
  - Critical bootstrap/activation paths gain deterministic unit coverage, reducing reliance on slow end-to-end scripts.
  - CI output clarifies which harness failed, shortening the feedback loop.
  - Shared fixtures reduce duplication between Rust and Python tests.

- **Negative / Risks:**
  - The directory rename requires touch-ups in existing scripts, IDE run configurations, and documentation.
  - Contributors must learn the taxonomy, and reviews need to police placement for a few weeks.
  - Running Python tests from a subdirectory may miss legacy tests until the migration completes; mitigated by performing the move in the same PR as the tooling updates.

## Implementation Notes

- Perform restructuring in stages (rename directories, update tooling, then move tests) to keep diffs reviewable.
- Introduce helper crates/modules under `tests/support/` to share temporary-directory setup between Rust and Python as soon as the taxonomy lands.
- Add `ruff` and `cargo fmt` hooks to ensure moved tests stay linted after the reorganisation.

## Status Tracking

- This ADR is **Accepted**. Directory restructuring, unit/integration coverage for the targeted modules, and the split CI/coverage jobs have landed; future adjustments will be tracked in follow-up ADRs if required.

## Alternatives Considered

- **Keep the current layout and document it informally.** Rejected because the `test/` vs `tests/` split is already causing confusion and does not solve the missing coverage gaps.
- **Create a monolithic Python integration test harness only.** Rejected because many FFI safety checks are cheaper to assert in Rust without spinning up subprocesses.
- **Adopt a coverage percentage gate.** Deferred until we have stable baselines; enforcing a percentage before addressing the structural issues would block unrelated work.
