# Test Suite Coverage Plan for codetracer-python-recorder

## Goals
- Provide lightweight code coverage signals for both the Rust and Python layers without blocking CI on initial roll-out.
- Enable engineers to inspect coverage reports for targeted modules (runtime activation, session bootstrap, Python facade helpers) while keeping runtimes acceptable.
- Lay groundwork for future gating (e.g., minimum coverage thresholds) once the numbers stabilise.

## Tooling Choices
- **Rust:** Use `cargo llvm-cov` to aggregate unit and integration test coverage. This tool integrates with `nextest` and produces both lcov and HTML outputs. It works with the existing `nix develop` environment once `llvm-tools-preview` is available (already pulled by rustup in Nix environment).
- **Python:** Use `pytest --cov` with the `coverage` plugin. Restrict collection to the `codetracer_python_recorder` package to avoid noise from site-packages. Generate both terminal summaries and Cobertura XML for upload.

## Execution Strategy
1. **Local Workflow**
   - Add convenience Just targets:
     - `just coverage-rust` → `uv run cargo llvm-cov --no-default-features --lcov --output-path target/coverage/rust.lcov` (also emit HTML under `target/coverage/html`).
     - `just coverage-python` → `uv run --group dev --group test pytest --cov=codetracer_python_recorder --cov-report=term --cov-report=xml:target/coverage/python.xml tests/python`.
   - Document usage in `tests/README.md` so developers can opt-in.

2. **CI Integration (non-blocking first pass)**
   - Extend `.github/workflows/ci.yml` with optional coverage steps that run after the primary test steps on one Python version (e.g., 3.12) to limit runtime.
   - Upload artefacts:
     - Rust: `rust-coverage.lcov` + zipped HTML report.
     - Python: `python-coverage.xml` + terminal summary (already in logs).
   - Mark coverage steps as `continue-on-error: true` initially so transient issues don’t fail the pipeline.

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

## Risks & Mitigations
- **Runtime overhead:** Coverage runs are slower. Mitigate by limiting to a single matrix entry and caching `target/coverage` directories if needed.
- **Report size:** HTML artefacts can be large. Compress before upload and prune historical runs as necessary.
- **PyO3 instrumentation quirks:** Ensure `cargo llvm-cov` runs with `--no-default-features` similar to existing `nextest` invocation to avoid mismatched Python symbols.
- **Coverage accuracy:** Python subprocess-heavy tests may under-report coverage. Supplement with targeted unit tests already added in Stage 4.

## Next Actions
- Implement the local Just targets and update documentation.
- Extend CI workflow with optional coverage steps (post-tests) and artefact upload.
- Align with the developer experience team before enforcing thresholds.
