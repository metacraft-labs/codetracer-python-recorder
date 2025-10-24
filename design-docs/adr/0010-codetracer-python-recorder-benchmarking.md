# 0010 – Codetracer Python Recorder Benchmarking

## Status
Proposed – pending review and implementation sequencing (target: post-configurable-trace-filter release).

## Context
- The Rust-backed `codetracer-python-recorder` now exposes configurable trace filters (WS1–WS6) and baseline micro/perf smoke benchmarks, but these are developer-only workflows with no CI visibility or historical tracking.
- Performance regressions are difficult to detect: Criterion runs produce local reports, the Python smoke benchmark is opt-in, and CI currently exercises only functional correctness.
- Product direction demands confidence that new features (filters, IO capture, PyO3 integration, policy changes) do not introduce unacceptable overhead or redaction slippage across representative workloads.
- We require an auditable, automated benchmarking strategy that integrates with existing tooling (`just`, `uv`, Nix flake, GitHub Actions/Jenkins) and surfaces trends to the team without burdening release cadence.

## Decision
We will build a first-class benchmarking suite for `codetracer-python-recorder` with three pillars:

1. **Deterministic harness coverage**
   - Preserve the existing Criterion microbench (`benches/trace_filter.rs`) and Python smoke benchmark, expanding them into a common `bench` workspace with reusable fixtures and scenario definitions (baseline, glob, regex, IO-heavy, auto-start).
   - Introduce additional Rust benches for runtime hot paths (scope resolution, redaction policy application, telemetry writes) under `codetracer-python-recorder/benches/`.
   - Add Python benchmarks (Pytest plugins + `pytest-benchmark` or custom timers) for end-to-end CLI runs, session API usage, and cross-process start/stop costs.

2. **Automated execution & artefacts**
   - Create a dedicated `just bench-all` (or extend `just bench`) command that orchestrates all benchmarks, produces structured JSON summaries (`target/perf/*.json`), and archives raw outputs (Criterion reports, flamegraphs when enabled).
   - Provide a stable JSON schema capturing metadata (git SHA, platform, interpreter versions), scenario descriptors, statistics (p50/p95/mean, variance), and thresholds.
   - Ship a lightweight renderer (`scripts/render_bench_report.py`) that compares current results against the latest baseline stored in CI artefacts.

3. **CI integration & historical tracking**
   - Add a continuous benchmark job (nightly and pull-request optional) that executes the suite inside the Nix shell (ensuring gnuplot/nodeps), uploads artefacts to GitHub Actions artefacts for long-term storage, and posts summary comments in PRs.
   - Maintain baseline snapshots in-repo (`codetracer-python-recorder/benchmarks/baselines/*.json`) refreshed on release branches after running on dedicated hardware.
   - Gate merges when regressions exceed configured tolerances (e.g., >5% slowdowns on primary scenarios) unless explicitly approved.

Supporting practices:
- Store benchmark configuration alongside code (`benchconfig.toml`) to keep scenarios versioned and reviewable.
- Ensure opt-in developer tooling (`just bench`) remains fast by allowing subset filters (e.g., `JUST_BENCH_SCENARIOS=filters,session`).

## Rationale
- **Consistency:** Centralising definitions and outputs ensures that local runs and CI share identical workflows, reducing “works on my machine” drift.
- **Observability:** Structured artefacts + historical storage let us graph trends, spot regressions early, and correlate with feature work.
- **Scalability:** By codifying thresholds and baselines, we can expand the suite without rethinking CI each time (e.g., adding memory benchmarks).
- **Maintainability:** Versioned configuration and scripts avoid ad-hoc shell pipelines and make it easy for contributors to extend benchmarks.

## Consequences
Positive:
- Faster detection of performance regressions and validation of expected improvements.
- Shared language for performance goals (scenarios, metrics, thresholds) across Rust and Python components.
- Developers gain confidence via `just bench` parity with CI, plus local comparison tooling.

Negative / Risks:
- Running the full suite may increase CI time; we mitigate by scheduling nightly runs and allowing PR opt-in toggles.
- Maintaining baselines requires disciplined updates whenever we intentionally change performance characteristics.
- Additional scripts and artefacts introduce upkeep; we must document workflows and automate cleanup.

Mitigations:
- Provide partial runs (`just bench --scenarios filters`, `pytest ... -k benchmark`) for quick iteration.
- Automate baseline updates via a `scripts/update_bench_baseline.py` helper with reviewable diffs.
- Document the suite in `docs/onboarding/trace-filters.md` (updated) and a new benchmarking guide.

## References
- `codetracer-python-recorder/benches/trace_filter.rs` (current microbench harness).
- `codetracer-python-recorder/tests/python/perf/test_trace_filter_perf.py` (Python smoke benchmark).
- `Justfile` (`bench` recipe) and `nix/flake.nix` (dev shell dependencies, now including gnuplot).
- Storage backend for historical data (settled: GitHub Actions artefacts).
