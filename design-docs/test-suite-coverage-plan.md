# Test Suite Coverage Plan for codetracer-python-recorder

## Goals
- Provide lightweight code coverage signals for both the Rust and Python layers without blocking CI on initial roll-out.
- Enable engineers to inspect coverage reports for targeted modules (runtime activation, session bootstrap, Python facade helpers) while keeping runtimes acceptable.
- Lay groundwork for future gating (e.g., minimum coverage thresholds) once the numbers stabilise.

## Tooling Choices
- **Rust:** Use `cargo llvm-cov` to aggregate unit and integration test coverage. This tool integrates with `nextest` and produces both lcov and HTML outputs. It works with the existing `nix develop` environment once `llvm-tools-preview` is available (already pulled by rustup in Nix environment).
- **Python:** Use `pytest --cov` with the `coverage` plugin. Restrict collection to the `codetracer_python_recorder` package to avoid noise from site-packages. Generate both terminal summaries and Cobertura XML for upload.

## Prerequisites & Dependencies
- Add `cargo-llvm-cov` to the dev environment so the Just targets and CI runners share the same binary. In the Nix shell, include the package and ensure the Rust toolchain exposes `llvm-tools-preview` or equivalent `llvm` binaries. The current dev shell ships `llvmPackages_latest.llvm`, making `llvm-cov`/`llvm-profdata` available without rustup components.
- Extend the UV `dev` dependency group with `pytest-cov` and `coverage[toml]` so Python coverage instrumentation is reproducible locally and in CI.
- Standardise coverage outputs under `codetracer-python-recorder/target/coverage` to keep artefacts inside the Rust crate. Use `target/coverage/{rust,python}` for per-language assets and a top-level `index.txt` to note the run metadata if needed later.

## Execution Strategy
1. **Local Workflow**
   - Add convenience Just targets that mirror the default test steps:
     - `just coverage-rust` → `LLVM_COV=$(command -v llvm-cov) LLVM_PROFDATA=$(command -v llvm-profdata) uv run cargo llvm-cov --manifest-path codetracer-python-recorder/Cargo.toml --no-default-features --nextest --lcov --output-path codetracer-python-recorder/target/coverage/rust/lcov.info`, followed by `cargo llvm-cov report --summary-only --json` to generate `summary.json` and a Python helper that prints a table mirroring the pytest coverage output. Document that contributors can run a second `cargo llvm-cov … --html --output-dir …` invocation when they need browsable reports because the CLI disallows combining `--lcov` and `--html` in a single run.
     - `just coverage-python` → `uv run --group dev --group test pytest --cov=codetracer_python_recorder --cov-report=term --cov-report=xml:codetracer-python-recorder/target/coverage/python/coverage.xml codetracer-python-recorder/tests/python`.
     - `just coverage` wrapper → runs the Rust step followed by the Python step so developers get both artefacts with one command, matching the eventual CI flow.
   - Ensure the commands create their output directories (`target/coverage/rust` and `target/coverage/python`) before writing results to avoid failures on first use.
   - Document the workflow in `codetracer-python-recorder/tests/README.md` (and reference the top-level `README` if needed) so contributors know when to run the coverage helpers versus the regular test splits.

2. **CI Integration (non-blocking first pass)**
   - Extend `.github/workflows/ci.yml` with optional `coverage-rust` and `coverage-python` jobs that depend on the primary test jobs and only run when `matrix.python-version == '3.12'` and `matrix.os == 'ubuntu-latest'` to avoid duplicate collection.
   - Reuse the Just targets so CI mirrors local behaviour. Inject `RUSTFLAGS`/`RUSTDOCFLAGS` from the test jobs’ cache to avoid rebuilding dependencies.
   - Publish artefacts via `actions/upload-artifact`:
     - Rust: `codetracer-python-recorder/target/coverage/rust/lcov.info`, the machine-readable `summary.json`, and optionally a gzipped HTML folder produced via a follow-up `cargo llvm-cov nextest --html --output-dir …` run in the same job.
     - Python: `codetracer-python-recorder/target/coverage/python/coverage.xml` for future parsing.
   - Mark coverage steps with `continue-on-error: true` during the stabilisation phase and note the run IDs in the job summary for quick retrieval.

3. **Reporting & Visualisation**
   - Use GitHub Actions artefacts for report retrieval.
   - Investigate integration with Codecov or Coveralls once the raw reports stabilise; defer external upload until initial noise is assessed.

## Incremental Roll-Out
1. Land Just targets and documentation so engineers can generate coverage locally.
2. Add CI coverage steps guarded by `if: matrix.python-version == '3.12'` to avoid duplicate work across versions.
3. Monitor runtimes and artefact sizes for a few cycles.
4. Once stable:
   - Remove `continue-on-error` and make coverage generation mandatory.
   - Introduce thresholds (e.g., fail if Rust line coverage < 70% or Python < 60%)—subject to discussion with the Runtime Tracing Team.

## Implementation Checklist
- [ ] Update development environment dependencies (`flake.nix`, `pyproject.toml`) to support coverage tooling out of the box.
- [ ] Add `just coverage-rust`, `just coverage-python`, and `just coverage` helpers with directory bootstrapping.
- [ ] Refresh documentation (`codetracer-python-recorder/tests/README.md` and top-level testing guide) with coverage instructions.
- [ ] Extend CI workflow with non-blocking coverage jobs and artefact upload.
- [ ] Review initial coverage artefacts to set baseline thresholds before enforcement.

## Risks & Mitigations
- **Runtime overhead:** Coverage runs are slower. Mitigate by limiting to a single matrix entry and caching `target/coverage` directories if needed.
- **Report size:** HTML artefacts can be large. Compress before upload and prune historical runs as necessary.
- **PyO3 instrumentation quirks:** Ensure `cargo llvm-cov` runs with `--no-default-features` similar to existing `nextest` invocation to avoid mismatched Python symbols.
- **Coverage accuracy:** Python subprocess-heavy tests may under-report coverage. Supplement with targeted unit tests already added in Stage 4.

## Next Actions
- Implement the local Just targets and update documentation.
- Extend CI workflow with optional coverage steps (post-tests) and artefact upload.
- Align with the developer experience team before enforcing thresholds.

_Status tracking lives in `design-docs/test-suite-coverage-plan.status.md`._
