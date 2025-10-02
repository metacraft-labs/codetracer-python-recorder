# Test Suite Coverage Plan Status

## Current Status
- ✅ Plan doc expanded with prerequisites, detailed Just targets, CI strategy, and an implementation checklist (see `design-docs/test-suite-coverage-plan.md`).
- 🚧 Implementation: add coverage tooling dependencies to the dev shell and UV groups so local + CI runners share the same setup.
- ⏳ Implementation: land `just coverage-*` helpers and update developer documentation.
- ⏳ Implementation: wire optional coverage jobs into `.github/workflows/ci.yml` with artefact uploads.
- ⏳ Assessment: capture baseline coverage numbers before proposing enforcement thresholds.

## Next Steps
1. Update environment dependencies for coverage (`cargo-llvm-cov`, `pytest-cov`, `coverage[toml]`).
2. Introduce the Just coverage commands and document how to use them.
3. Extend CI with non-blocking coverage collection and review the initial artefacts.
