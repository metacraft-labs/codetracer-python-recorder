# ADR 0003: Test Suite Governance
- **Status:** Accepted
- **Date:** 2025-10-02

## Context
Rust unit tests, Rust integration tests, and Python tests lived in confusing folders (`tests/` vs `test/`). Coverage overlapped in some spots and missed bootstrap/activation code entirely.

## Decision
Adopt a clear taxonomy and matching layout:
- Rust unit tests stay inline under `src/**`.
- Rust integration tests live in `tests/rust/`.
- Python tests live in `tests/python/`.
- Shared fixtures sit under `tests/support/`.
Every PR states which layer it touched, and changes to PyO3 plumbing require Rust coverage while Python-facing changes require pytest coverage.

We renamed directories, added `tests/README.md`, and split CI steps (`cargo nextest …`, `pytest tests/python`). Focused tests now cover bootstrap, output paths, activation controller, and Python helpers.

## Consequences
- 👍 Easier onboarding, clearer failures in CI, less duplicate code.
- 👎 Short-term churn in scripts/IDE configs while people learn the new paths.

## Implementation notes
- Move files + update tooling in the same PR to avoid gaps.
- Share fixtures early so both languages use the same helpers.
- Keep doctests disabled unless they run without CPython.
