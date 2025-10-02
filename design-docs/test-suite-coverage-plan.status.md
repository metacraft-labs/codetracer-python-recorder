# Test Suite Coverage Plan Status

## Current Status
- ✅ Plan doc expanded with prerequisites, detailed Just targets, CI strategy, and an implementation checklist (see `design-docs/test-suite-coverage-plan.md`).
- ✅ Implementation: coverage dependencies added to the dev shell (`flake.nix`) and UV groups (`pyproject.toml`).
- ✅ Implementation: `just coverage-*` helpers landed with matching documentation in `codetracer-python-recorder/tests/README.md`.
- ⏳ Implementation: wire optional coverage jobs into `.github/workflows/ci.yml` with artefact uploads.
- ⏳ Assessment: capture baseline coverage numbers before proposing enforcement thresholds.

## Next Steps
1. Extend CI with non-blocking coverage collection and review the initial artefacts.
2. Capture baseline coverage numbers ahead of enforcement proposals.
