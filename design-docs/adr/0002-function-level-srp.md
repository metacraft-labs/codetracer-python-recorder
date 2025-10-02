# ADR 0002: Function-Level SRP
- **Status:** Accepted
- **Date:** 2025-10-15

## Context
`start_tracing`, `RuntimeTracer::on_py_start/on_line/on_py_return`, and Python`s `session.start` each cram validation, activation logic, frame poking, and logging into long blocks. That makes fixes risky.

## Decision
Turn those hotspots into thin coordinators that call focused helpers:
- Bootstrap helpers prep directories, formats, and metadata before `start_tracing` proceeds.
- `frame_inspector` handles unsafe frame access; `ActivationController` owns gating.
- `value_capture` records arguments, scopes, and returns.
- Logging/error helpers keep messaging consistent.
Python mirrors this with `_validate_trace_path`, `_coerce_format`, etc.
APIs stay the same for callers.

## Consequences
- 👍 Smaller functions, better unit tests, clearer error handling.
- 👎 More helper modules to navigate; moving unsafe code needs care.

## Guidance
- Extract one concern at a time, keep helpers `pub(crate)` when possible.
- Wrap unsafe code in documented helpers and add unit tests for each new module.
- Run `just test` + fixture comparisons after every extraction.

## Follow up
- Track work in the function-level SRP plan and watch performance to ensure the extra indirection stays cheap.
