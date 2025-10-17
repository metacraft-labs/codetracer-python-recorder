# Codetracer Python Recorder Benchmarking – Implementation Plan

Linked ADR: `design-docs/adr/0010-codetracer-python-recorder-benchmarking.md`

Target window: Post-configurable-trace-filter WS6 (tentatively WS7–WS8)

## Goals
- Deliver a comprehensive benchmarking suite covering hot Rust paths and Python end-to-end workflows.
- Integrate the suite with CI to surface regression reports and maintain historical performance baselines.
- Provide developer-friendly tooling (`just` recipes, scripts) for local reproduction and analysis.

## Non-Goals
- Real-time production telemetry ingestion (future project).
- Automated hardware provisioning for benchmark runners (assume existing CI hosts).

## Workstreams

### WS1 – Benchmark Foundations
- Audit existing microbench (Criterion) and Python smoke tests; identify shared fixtures and gaps.
- Define canonical benchmark scenarios and metadata schema (`benchconfig.toml`).
- Introduce `codetracer-python-recorder/benchmarks/` workspace with reusable dataset builders.
- Extend Rust benches:
  - `trace_filter.rs` (reuse, parameterise scenario loading from `benchconfig`).
  - New benches for runtime modules: `engine_resolve`, `value_policy`, `session_bootstrap`.
- Add Python benchmarks:
  - Use `pytest-benchmark` or custom timer harness to measure CLI startup, session API, filter application, metadata generation.
  - Emit JSON traces under `target/perf/python/*.json`.
- Update `Justfile` (`bench` → `bench-core`, add `bench-all`) to run Rust + Python suites with scenario filters.
- Ensure Nix dev shell contains required tooling (gnuplot, pytest-benchmark).
- Tune Criterion configuration (sample count, warm-up, flat sampling) to control noise, leveraging gnuplot for local visualisation.

### WS2 – Result Aggregation & Baselines
- Implement `scripts/render_bench_report.py` to summarise results and compare against a baseline JSON.
- Define JSON schema (`bench-schema.json`) capturing:
  - Git metadata (SHA, branch).
  - System info (OS, CPU, interpreter versions, PyO3 flags).
  - Scenario metrics (mean, stddev, p95, sample counts).
- Seed initial baselines (`benchmarks/baselines/*.json`) using controlled runs on CI hardware.
- Create helper `scripts/update_bench_baseline.py` for refreshing baselines.
- Document storage conventions in `docs/onboarding/benchmarking.md`.

### WS3 – CI Integration
- Add GitHub Actions (or Jenkins) workflow `bench.yml` with matrix support (Linux x86_64 first, macOS optional).
- Steps:
  1. Enter Nix dev shell (flakes).
  2. Run `just bench-all`.
  3. Upload `target/perf` directory and raw Criterion reports as GitHub Actions artefacts.
  4. Execute `render_bench_report.py` vs baseline; fail job if thresholds exceeded.
  5. Post summary comment on PRs (via `gh` CLI or bot).
- Schedule nightly benchmark runs on `main` to capture trends and update time-series storage (optional S3 upload).
- Ensure workflow caches `~/.cargo`/`uv` to control runtime.

### WS4 – Reporting & Tooling UX
- Build `scripts/bench_report_html.py` (optional) to render static HTML charts using existing JSON (for sharing).
- Add `docs/onboarding/benchmarking.md` with:
  - Scenario catalogue and interpretation guidance.
  - Instructions for updating baselines and triaging regressions.
- Enhance `just bench` to accept `SCENARIOS` env var and `--compare` flag (local vs baseline diff).
- Provide pre-commit hook (optional) reminding devs to run `just bench` before merging perf-sensitive changes.

### WS5 – Guard Rails & Maintenance
- Define regression thresholds per scenario (e.g., `baseline`: 5%, `filter_glob`: 7%).
- Implement allowlist mechanism for temporary exceptions (`benchmarks/exceptions.yaml`).
- Integrate results into release checklist (CI gating, baseline refresh).
- Establish ownership (`CODEOWNERS`) for benchmarking artefacts.

## Deliverables
- ADR 0010 (this plan’s prerequisite) – ✅
- Updated `Justfile` commands and benchmarking scripts.
- JSON schema, baselines, and reporting scripts.
- CI workflow with artefact uploads and regression checks.
- Documentation: onboarding guide, contribution guidelines for benchmarks.

## Risks & Mitigations
- **CI flakiness**: Variability due to shared hardware.
  - Mitigate with warm-up passes, controlled CPU governor, and trend-based thresholds (use median of multiple runs).
- **Developer friction**: Longer local runs.
  - Provide targeted scenario filters and guidance on when to run full suite.
- **Baseline drift**: Hard to keep in sync with intentional perf changes.
  - Use explicit PRs updating baselines with context + review; automate baseline capture script to reduce manual error.

## Open Questions
- Storage backend for historical data (GitHub artefacts vs S3/GCS).
- Whether to include memory allocations and binary size metrics in the first iteration.
- Potential integration with external dashboards (e.g., Grafana, BuildBuddy).

## Timeline (tentative, assuming 2-week sprints)
- WS1: 1 sprint – scaffolding and expanded harnesses.
- WS2: 0.5 sprint – aggregation tooling.
- WS3: 1 sprint – CI workflow & artefact management.
- WS4: 0.5 sprint – documentation + UX improvements.
- WS5: 0.5 sprint – guard rails and maintenance guidelines.

Total: ~3.5 sprints post-configurable-trace-filter release.
