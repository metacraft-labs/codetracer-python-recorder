# Codetracer Python Recorder Architecture Refactor – Implementation Plan

## Overview
We will refactor the `codetracer-python-recorder` crate to reinforce single-responsibility boundaries and reduce coupling among runtime tracing, policy, monitoring, and diagnostics layers. The work follows ADR 0011 and deliberately excludes the recently refactored IO capture pipeline (`src/runtime/io_capture/`).

## Goals
- Ensure large modules (`runtime/mod.rs`, `trace_filter/config.rs`, `trace_filter/engine.rs`, `monitoring/tracer.rs`, `logging.rs`, `policy.rs`, `session/bootstrap.rs`) each own a focused concern with cohesive helpers.  
- Preserve existing public APIs (Rust crate exports and Python bindings) while internally re-organising responsibilities.  
- Enable targeted unit testing by isolating IO, parsing, caching, and lifecycle logic.  
- Maintain runtime performance and behaviour (activation gating, telemetry, failure injection, trace filtering).

## Concept-to-file Mapping

| Concept | Current location(s) | Target location(s) |
| --- | --- | --- |
| Trace filter configuration models & defaults | `codetracer-python-recorder/src/trace_filter/config.rs` | `codetracer-python-recorder/src/trace_filter/model.rs`, `.../loader.rs` |
| Trace filter file IO & aggregation | `codetracer-python-recorder/src/trace_filter/config.rs` | `codetracer-python-recorder/src/trace_filter/loader.rs` |
| Trace filter summaries for metadata | `codetracer-python-recorder/src/trace_filter/config.rs` | `codetracer-python-recorder/src/trace_filter/summary.rs` |
| Trace filter runtime engine & cache | `codetracer-python-recorder/src/trace_filter/engine.rs` | `codetracer-python-recorder/src/trace_filter/engine/mod.rs`, `.../engine/resolution.rs` |
| Policy data model & updates | `codetracer-python-recorder/src/policy.rs` | `codetracer-python-recorder/src/policy/model.rs` |
| Policy environment parsing | `codetracer-python-recorder/src/policy.rs` | `codetracer-python-recorder/src/policy/env.rs` |
| Policy PyO3 bindings | `codetracer-python-recorder/src/policy.rs` | `codetracer-python-recorder/src/policy/ffi.rs` |
| Logging: logger, filter specs, destinations | `codetracer-python-recorder/src/logging.rs` | `codetracer-python-recorder/src/logging/logger.rs` |
| Logging: metrics sink | `codetracer-python-recorder/src/logging.rs` | `codetracer-python-recorder/src/logging/metrics.rs` |
| Logging: error trailer emission | `codetracer-python-recorder/src/logging.rs` | `codetracer-python-recorder/src/logging/trailer.rs` |
| Session bootstrap filesystem prep | `codetracer-python-recorder/src/session/bootstrap.rs` | `codetracer-python-recorder/src/session/bootstrap/filesystem.rs` |
| Session bootstrap metadata capture | `codetracer-python-recorder/src/session/bootstrap.rs` | `codetracer-python-recorder/src/session/bootstrap/metadata.rs` |
| Session bootstrap filter loading | `codetracer-python-recorder/src/session/bootstrap.rs` | `codetracer-python-recorder/src/session/bootstrap/filters.rs` |
| Monitoring tracer trait & types | `codetracer-python-recorder/src/monitoring/tracer.rs` | `codetracer-python-recorder/src/monitoring/api.rs` |
| Monitoring install/uninstall plumbing | `codetracer-python-recorder/src/monitoring/tracer.rs` | `codetracer-python-recorder/src/monitoring/install.rs`, `.../callbacks.rs` |
| Runtime tracer lifecycle management | `codetracer-python-recorder/src/runtime/mod.rs` | `codetracer-python-recorder/src/runtime/tracer/lifecycle.rs` |
| Runtime tracer event handlers | `codetracer-python-recorder/src/runtime/mod.rs` | `codetracer-python-recorder/src/runtime/tracer/events.rs` |
| Runtime tracer IO coordination | `codetracer-python-recorder/src/runtime/mod.rs` | `codetracer-python-recorder/src/runtime/tracer/io.rs` |
| Runtime tracer filter cache & policy integration | `codetracer-python-recorder/src/runtime/mod.rs` | `codetracer-python-recorder/src/runtime/tracer/filtering.rs` |
| Python session orchestration | `codetracer-python-recorder/codetracer_python_recorder/session.py` | `codetracer-python-recorder/codetracer_python_recorder/session.py` (imports updated to new Rust facades) |
| Python CLI argument resolution | `codetracer-python-recorder/codetracer_python_recorder/cli.py` | `codetracer-python-recorder/codetracer_python_recorder/cli.py` (uses refactored bootstrap/service APIs) |

## Scope
- Rust crate `codetracer-python-recorder`, excluding `src/runtime/io_capture/`.  
- Python package glue (`codetracer_python_recorder/cli.py`, `codetracer_python_recorder/session.py`) only to the extent necessary to align imports with new Rust module facades.  
- Existing unit/integration tests; add coverage as required by new abstractions.

## Non-Goals
- Functional changes to tracing behaviour, policy semantics, or trace output formats.  
- Revisiting IO capture mechanics or Python auto-start logic beyond import adjustments.  
- Altering external CLI or Python API signatures (behavioural parity is mandatory).

## Milestones

### 1. Trace Filter Decomposition
- Introduce submodules: `trace_filter::model` (directives, value actions), `trace_filter::loader` (TOML parsing, source aggregation), `trace_filter::summary`, and move cache-independent helpers out of `engine.rs`.  
- Refactor `TraceFilterEngine` to depend on compiled rule structs imported from new modules; keep resolver cache logic local.  
- Update callers (`session/bootstrap.rs`, `runtime/mod.rs`) to use the facade module.  
- Extend/adjust unit tests covering filter parsing and resolution.  
- Run `just test`.

### 2. Policy and Logging Separation
- Split `policy.rs` into `policy::model` (data structures, in-memory updates), `policy::env` (environment parsing), and `policy::ffi` (PyO3 functions). Create a top-level `policy/mod.rs` facade exporting current names.  
- Extract logging responsibilities into `logging::logger` (FilterSpec, destination management), `logging::metrics`, `logging::trailer`, with a facade orchestrating policy application and structured logging helpers.  
- Update call sites (`RuntimeTracer::finish`, `session::start_tracing`, tests) to use new facades.  
- Refresh policy/logging unit tests; add coverage for failure cases in new modules.  
- Run `just test`.

### 3. Session Bootstrap Refactor
- Break `TraceSessionBootstrap` into submodules (`bootstrap::filesystem`, `bootstrap::metadata`, `bootstrap::filters`) maintaining a thin orchestrator struct.  
- Provide dedicated unit tests for each submodule (e.g., metadata extraction, filter discovery).  
- Update Python wrappers (`session.py`, `cli.py`) if import statements change; ensure behaviour remains identical.  
- Verify `just test` and targeted CLI smoke tests (`just run` scenario if available).

### 4. Monitoring Plumbing Cleanup
- Move the `Tracer` trait definition into `monitoring::api`; encapsulate global install/uninstall state in `monitoring::install`.  
- Generate callback registration via a declarative table or macro to replace the duplicated functions in `monitoring/tracer.rs`.  
- Ensure the public functions `install_tracer`, `uninstall_tracer`, and `flush_installed_tracer` remain accessible from `crate::monitoring`.  
- Update or add tests validating callback dispatch and disable-on-error behaviour.  
- Run `just test`.

### 5. Runtime Tracer Modularisation
- Introduce collaborators for lifecycle management (trace file setup, teardown), event handling (py_start/line/return), filter cache lookup, and IO coordination.  
- Refactor `RuntimeTracer` to compose these collaborators, keeping state injection explicit and eliminating unrelated helper functions from the main impl.  
- Ensure failure injection hooks, telemetry counters, activation gating, and existing public methods (`begin`, `install_io_capture`, `flush`, `finish`) behave identically.  
- Update `src/runtime/mod.rs` unit tests and add coverage for new components.  
- Re-run `just test` plus targeted integration tests if available.

### 6. Integration and Cleanup
- Harmonise module exports, update documentation comments referencing moved code, and ensure Python packaging metadata/build scripts still resolve module paths.  
- Review for dead imports or obsolete helpers left behind after splits.  
- Run the full test suite (`just test`), optionally `cargo fmt`/`cargo clippy` if part of CI requirements.  
- Prepare follow-up documentation updates or status reports; confirm with stakeholders that milestones meet ADR intent.

## Testing Strategy
- Incrementally adjust existing unit tests to target new modules.  
- Add focused tests where previously impossible (e.g., loader-only tests without touching runtime).  
- Maintain or enhance integration tests covering start/stop tracing flows.  
- Execute `just test` after each milestone; add performance smoke benchmarks if regressions are suspected.

## Risks & Mitigations
- **Regression risk:** Break tracing lifecycle when splitting modules. Mitigate with exhaustive unit tests and incremental commits.  
- **Merge conflicts:** Large file churn may collide with parallel work. Communicate schedule early, stage PRs sequentially, and land high-churn files first.  
- **Performance impact:** Additional abstraction layers could add overhead. Benchmark hot paths after Milestones 4 and 5; profile if slowdowns exceed 5 %.  
- **Doc drift:** Architectural docs may become outdated. Schedule documentation updates during Milestone 6.

## Rollout & Sign-off
- Track milestones via status files (e.g., `codetracer-architecture-refactor-implementation-plan.status.md`).  
- Flip ADR 0011 to **Accepted** once Milestone 6 completes and maintainers confirm no behavioural regressions.  
- Announce completion to stakeholders (runtime tracing users, DX tooling) and note any follow-up cleanups.
