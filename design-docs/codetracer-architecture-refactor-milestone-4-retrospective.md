# Codetracer Architecture Refactor – Milestone 4 Retrospective

- **Milestone window:** 2025‑02‑17 → 2025‑03‑01  
- **Scope recap:** Detangle `monitoring` so `sys.monitoring` plumbing lives in cohesive modules (`api`, `callbacks`, `install`) and eliminate hand-rolled callback registration/teardown logic while preserving the existing public facade.

## Outcomes
- Added a declarative `CALLBACK_SPECS` table plus helper APIs to drive callback registration/unregistration, replacing ~30 duplicate branches.  
- Centralised tracer state and error handling in `monitoring::callbacks`, ensuring panic-to-PyErr conversion, policy-driven disable flows, and callback execution share the same instrumentation.  
- Moved install/teardown logic into `monitoring::install`, leaving `monitoring::tracer` as a compatibility shim; consumers still import `install_tracer` et al. unchanged.  
- `just test` (Rust `cargo nextest` + Python `pytest`) passes post-refactor, confirming behavioural parity; one existing perf test remained skipped as expected.

## What Went Well
- Table-driven metadata drastically simplified maintenance—adding or removing CPython events is now a single-row change.  
- Co-locating global state with callback helpers removed redundant locking/unwrap patterns spread across modules.  
- Incremental updates to the status tracker kept context handy when the work paused between sessions.

## Challenges & Mitigations
- Adapting PyO3 wrappers required careful lifetime handling; switching helper factories to accept `Bound<'py, PyModule>` avoided compile-time churn.  
- Ensuring disable-on-error flows still reached teardown code meant delegating to `install::uninstall_locked`; unit paths relied on shared helpers to avoid divergence.  
- Multiple modules touched by the split increased the risk of import regressions. Running `cargo fmt` and `just test` after each major change caught mistakes early.

## Follow-Ups
1. Update developer docs (README/AGENTS) once Milestone 5 lands so the new monitoring structure is reflected in onboarding material.  
2. Revisit milestone test coverage to see if table-driven registration merits additional unit tests (e.g., verifying `CALLBACK_SPECS` completeness via assertions).  
3. Proceed to Milestone 5 (runtime tracer modularisation) using the newly isolated install/callback modules as building blocks.  
4. Capture any stakeholder feedback and incorporate into ADR 0011 before final acceptance.*** End Patch
