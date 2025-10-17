# ADR 0011: Codetracer Python Recorder Architecture Refactor

- **Status:** Proposed
- **Date:** 2025-02-14
- **Deciders:** codetracer recorder maintainers
- **Consulted:** DX tooling crew, Runtime tracing stakeholders
- **Informed:** Replay consumers, Support engineering

## Context
- `RuntimeTracer` (`codetracer-python-recorder/src/runtime/mod.rs`) has grown into a god object: it wires monitoring callbacks, trace file lifecycle, IO draining, policy enforcement, telemetry, and trace-filter integration inside a single 2 600+ line module.
- `trace_filter` files (`src/trace_filter/config.rs`, `engine.rs`) mix data modelling, on-disk parsing, default resolution, runtime caching, and PyO3-facing summaries, making it hard to isolate changes or test components individually.
- Monitoring plumbing (`src/monitoring/tracer.rs`) duplicates `sys.monitoring` registration boilerplate across >14 callback wrappers and stores the global tracer instance alongside policy decisions, reducing cohesion.
- Policy and diagnostics code (`src/policy.rs`, `src/logging.rs`) couple configuration models, env parsing, PyO3 bindings, metrics, file IO, and error-trailer logic in single modules.
- Python glue (`codetracer_python_recorder/cli.py`, `codetracer_python_recorder/session.py`) pulls details from the monolithic Rust modules, limiting our ability to present slimmer APIs or reuse bootstrapping logic elsewhere.
- The team wants stricter adherence to the single-responsibility principle and lower coupling so future features (e.g., new policy toggles, additional monitoring events, alternative telemetry sinks) can be added with minimal risk.

## Problem
Large, multi-purpose modules make the recorder difficult to extend and review. Specific issues include:
- Testing isolated behaviours (e.g., trace-filter IO errors, policy inheritance) requires instantiating heavyweight structs because responsibilities are intertwined.
- Introducing new tracing behaviours often touches unrelated code, increasing the chance of regression (e.g., editing `RuntimeTracer` for filter tweaks while interfering with IO teardown).
- Reusing infrastructure (policy parsing, bootstrap metadata, logging) in other crates or integration tests is impractical because functionality is not encapsulated.
- The current code layout obscures high-level architecture, slowing onboarding and complicating code ownership boundaries.

## Decision
We will modularise the recorder around cohesive responsibilities while preserving existing external APIs and without re-touching the recently refactored IO capture pipeline (`src/runtime/io_capture/`).

1. **Trace Filter Layering**  
   - Extract configuration models, parsing/aggregation, and runtime compilation into distinct submodules (e.g., `trace_filter::model`, `::loader`, `::engine`, `::summary`).  
   - Keep file IO and TOML parsing contained in loader modules, letting runtime components depend only on pure data structures.  
   - Preserve the current public API (`TraceFilterConfig`, `TraceFilterEngine`) via a facade module to avoid churn for callers.

2. **Policy & Diagnostics Separation**  
   - Split policy data structures from environment parsing and PyO3 bindings, yielding `policy::model`, `policy::env`, and `policy::ffi`.  
   - Partition logging into `logging::logger` (FilterSpec parsing, writer management), `logging::metrics`, and `logging::trailer`, with a top-level facade that applies policies.  
   - Ensure policy updates flow through a narrow interface consumed by both Rust and Python entry points.

3. **Session Bootstrap Decomposition**  
   - Break `TraceSessionBootstrap` helpers into filesystem preparation, metadata capture, and filter loading modules.  
   - Provide a lightweight bootstrap service consumed by both Rust (`start_tracing`) and Python CLI/session wrappers, improving reuse and testability.

4. **Monitoring Callback Plumbing**  
   - Move the `Tracer` trait and its helpers into dedicated modules (`monitoring::api`, `monitoring::install`).  
   - Replace duplicated callback registration functions with table-driven or macro-generated wrappers while keeping `install_tracer`, `uninstall_tracer`, and `flush_installed_tracer` signatures intact.

5. **Runtime Tracer Orchestration**  
   - Factor `RuntimeTracer` responsibilities into focused collaborators handling lifecycle management, event handling, filter caching, and IO coordination.  
   - Maintain behavioural equivalence (activation gating, telemetry, failure injection) but reduce per-function responsibilities and clarify dependencies via constructor injection.

## Consequences
- **Benefits:**  
  - Easier to reason about components and review changes; smaller files have clearer ownership and targeted unit tests.  
  - Reduced coupling between layers unlocks future features (e.g., alternative log sinks, additional monitoring events) without large-scale edits.  
  - Python-facing APIs rely on slimmer Rust facades, improving maintainability for CLI and embedding scenarios.
- **Costs:**  
  - Significant module churn requires careful coordination of imports, visibility, and re-exports.  
  - Temporary refactor scaffolding increases short-term complexity; we must keep commits small and well-tested.  
  - Documentation and developer onboarding material must be updated to reflect the new layout.
- **Risks:**  
  - Behavioural regressions if lifecycle or policy logic is incorrectly reassembled—mitigated by incremental changes with exhaustive unit/integration tests.  
  - Merge conflicts with parallel workstreams touching large files; we will stage the refactor and communicate timelines.  
  - Potential performance regressions if new abstractions add indirection; we will benchmark hot paths after each milestone.

## Alternatives
- **Targeted cleanups only:** Fixing individual hotspots without broader modularisation would leave the overarching coupling unaddressed and perpetuate inconsistent boundaries.  
- **Full rewrite:** Starting from scratch would be risky for existing integrations and offers little incremental value compared to methodical refactoring.

## Rollout
1. Land the modularisation in staged PRs following the implementation plan, keeping behavioural changes isolated per milestone.  
2. Maintain compatibility with current Python APIs and crate exports; adjust import paths gradually with deprecation windows if needed.  
3. Update architectural documentation and developer guides once core milestones complete.  
4. Flip ADR status to **Accepted** after the implementation plan reaches testing sign-off and owners agree the new structure delivers the intended cohesion.
