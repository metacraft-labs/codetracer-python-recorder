# ADR 0001: File-Level Single Responsibility Refactor

- **Status:** Proposed
- **Date:** 2025-10-01
- **Deciders:** Platform / Runtime Tracing Team
- **Consulted:** Python Tooling WG, Developer Experience WG

## Context

The codetracer Python recorder crate has evolved quickly and several source files now mix unrelated concerns:
- [`src/lib.rs`](../../codetracer-python-recorder/src/lib.rs) hosts PyO3 module wiring, global logging setup, tracing session state, and filesystem validation in one place.
- [`src/runtime/tracer/runtime_tracer.rs`](../../codetracer-python-recorder/src/runtime/tracer/runtime_tracer.rs) (formerly `src/runtime_tracer.rs`) interleaves activation gating, writer lifecycle control, PyFrame helpers, and Python value encoding logic, making it challenging to test or extend any portion independently.
- [`src/tracer.rs`](../../codetracer-python-recorder/src/tracer.rs) combines sys.monitoring shim code with the `Tracer` trait, callback registration, and global caches.
- [`codetracer_python_recorder/api.py`](../../codetracer-python-recorder/codetracer_python_recorder/api.py) mixes format constants, backend interaction, context manager ergonomics, and environment based auto-start side effects.

This violates the Single Responsibility Principle (SRP) at the file level, obscures ownership boundaries, and increases the risk of merge conflicts and regressions. Upcoming work on richer value capture and optional streaming writers will add more logic to these files unless we carve out cohesive modules now.

## Decision

We will reorganise both the Rust crate and supporting Python package so that each file covers a single cohesive topic and exposes a narrow interface. Concretely:
1. Restrict `src/lib.rs` to PyO3 module definition and `pub use` re-exports. Move logging configuration into `src/logging.rs` and tracing session lifecycle into `src/session.rs`.
2. Split the current runtime tracer into a `runtime` module directory with dedicated files for activation control, value encoding, and output file management. The façade in `runtime/mod.rs` will assemble these pieces and expose the existing `RuntimeTracer` API.
3. Introduce a `monitoring` module directory that separates sys.monitoring primitive bindings (`EventId`, `ToolId`, registration helpers) from the `Tracer` trait and callback dispatch logic.
4. Decompose the Python helper package by moving session state management into `session.py`, format constants and validation into `formats.py`, and environment auto-start into `auto_start.py`, while keeping public functions surfaced through `api.py` and `__init__.py`.

These changes are mechanical reorganisations—no behavioural changes are expected. Public Rust and Python APIs must remain source compatible during the refactor.

## Consequences

- **Positive:**
  - Easier onboarding for new contributors because each file advertises a single purpose.
  - Improved unit testability; e.g., Python value encoding can be tested without instantiating the full tracer.
  - Lower merge conflict risk: teams can edit activation logic without touching writer code.
  - Clearer extension points for upcoming streaming writer and richer metadata work.
- **Negative / Risks:**
  - Temporary churn in module paths may invalidate outstanding branches; mitigation is to stage work in small, reviewable PRs.
  - Developers unfamiliar with Rust module hierarchies will need guidance to update `mod` declarations and `use` paths correctly.
  - Python packaging changes require careful coordination to avoid circular imports when moving auto-start logic.

## Implementation Guidelines for Junior Developers

1. **Work Incrementally.** Aim for small PRs (≤500 LOC diff) that move one responsibility at a time. After each PR run `just test` and ensure all linters stay green.
2. **Preserve APIs.** When moving functions, re-export them from their new module so that existing callers (Rust and Python) compile without modification in the same PR.
3. **Add Focused Tests.** Whenever a helper is extracted (e.g., value encoding), add or migrate unit tests that cover its edge cases.
4. **Document Moves.** Update doc comments and module-level docs to reflect the new structure. Remove outdated TODOs or convert them into follow-up issues.
5. **Coordinate on Shared Types.** When evolving the `runtime::tracer` modules, agree on ownership for shared structs (e.g., `RuntimeTracer` remains re-exported from `runtime/mod.rs`). Use `pub(crate)` to keep internals encapsulated.
6. **Python Imports.** After splitting the Python modules, ensure `__all__` in `__init__.py` continues to export the public API. Use relative imports to avoid accidental circular dependencies.
7. **Parallel Work.** Follow the sequencing from `design-docs/file-level-srp-refactor-plan.md` to know when tasks can proceed in parallel.

## Testing Strategy

- Run `just test` locally before submitting each PR.
- Add targeted Rust tests for new modules (e.g., `activation` and `value_encoder`).
- Extend Python tests to cover auto-start logic and the context manager after extraction.
- Compare trace outputs against saved fixtures to ensure refactors do not alter serialized data.

## Alternatives Considered

- **Leave the layout as-is:** rejected because it impedes planned features and increases onboarding cost.
- **Large rewrite in a single PR:** rejected due to high risk and code review burden.

## Follow-Up Actions

- After completing the refactor, update architecture diagrams in `design-docs` to match the new module structure.
- Schedule knowledge-sharing sessions for new module owners to walk through their areas.
