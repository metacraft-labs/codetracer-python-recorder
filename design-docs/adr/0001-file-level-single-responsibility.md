# ADR 0001: File-Level Single Responsibility Refactor
- **Status:** Accepted
- **Date:** 2025-10-01

## Context
Big files (`src/lib.rs`, `runtime_tracer.rs`, `tracer.rs`, and `codetracer_python_recorder/api.py`) each juggle multiple jobs. Upcoming value-capture + streaming work would make that worse.

## Decision
Break the crate and Python package into focused modules:
- `src/lib.rs` keeps PyO3 wiring only; logging goes to `logging.rs`, session lifecycle to `session.rs`.
- `runtime/` becomes a directory with `mod.rs`, `activation.rs`, `value_encoder.rs`, `output_paths.rs`.
- `monitoring/` holds sys.monitoring types in `mod.rs` and the `Tracer` trait/dispatcher in `tracer.rs`.
- Python package gains `session.py`, `formats.py`, `auto_start.py`; `api.py` stays as the façade.
Public APIs remain unchanged—files just move.

## Consequences
- 👍 Easier onboarding, smaller testable units, fewer merge conflicts, clear extension points.
- 👎 Short-term churn in module paths and imports; coordinate PRs carefully.

## Notes for implementers
- Move code in small PRs, re-export functions so callers keep working.
- Add/adjust unit tests when extracting helpers.
- Update docs/comments alongside code moves.
- Follow the sequencing in the file-level SRP plan for parallel work.

## Testing
- Run `just test` after each move.
- Compare trace fixtures to ensure behaviour stays the same.
