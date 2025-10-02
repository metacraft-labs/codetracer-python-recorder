# ADR 0004: Error Handling Policy
- **Status:** Accepted
- **Date:** 2025-10-02

## Context
Errors currently bubble up as ad-hoc `PyRuntimeError`s or panics that can crash the host process. Messages vary, nothing is classified, and trace files can be left half-written.

## Decision
Adopt a single policy driven by a new `recorder-errors` crate and matching Python exceptions:
- `RecorderError` carries `{kind, code, message, context}` with a tiny enum for codes.
- Kinds map to fixed exit codes: Usage=2, Environment=10, Target=20, Internal=70.
- All PyO3 entry points go through a wrapper that catches panics and turns errors into `RecorderError` subclasses (`UsageError`, etc.).
- Output writing stages files in a temp dir and either renames atomically or marks the trace as partial.
- A runtime policy switch controls whether we abort or just disable tracing on failure.
- Logging uses structured records; optional JSON diagnostics attach `run_id`, `trace_id`, and `error_code`.
- `panic!`, `unwrap`, and `expect` are banned outside guarded helpers; macros (`usage!`, `enverr!`, `bug!`) replace them.

## Consequences
- 👍 Stable error codes, no more interpreter-aborting panics, consistent logs for tooling.
- 👎 More plumbing to retrofit existing call sites and slightly more disk I/O for atomic writes.

## Rollout notes
- Land the crate, swap call sites to `RecorderResult`, and update Python wrappers to raise the new hierarchy.
- Add tests for every error code, simulated IO failures, policy toggles, and panic containment.
- Document the guarantees in README + crate docs: no stdout noise, atomic (or explicitly partial) traces, stable codes per minor version.
