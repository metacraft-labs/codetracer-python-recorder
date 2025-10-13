# `codetracer-python-recorder` PyPI Release – Status

## Workstream 1 – Package metadata & repository hygiene
- ✅ Tightened `pyproject.toml` metadata (Python 3.12–3.13 targets, classifiers, README long description, license file).
- ✅ Added maturin sdist include/exclude rules and a dedicated LICENSE copy for the crate.
- ✅ Introduced `scripts/check_recorder_version.py` and wired it into CI to enforce version parity.
- ✅ Documented supported environments in `codetracer-python-recorder/README.md` and added a release checklist under `design-docs/`.

## Workstream 2 – Build & test enhancements
- ✅ Updated `Justfile` build targets to emit wheels and sdists and added a `smoke-wheel` recipe that installs artefacts via an isolated venv.
- ✅ Added `scripts/select_recorder_artifact.py` to choose interpreter-compatible wheels and wired the smoke test to use it.
- ✅ Expanded the Python API tests to cover end-to-end tracing via `start`/`stop` (`test_start_emits_trace_files`).
- ✅ Verified `just py-test` passes with the new coverage.

## Workstream 3 – Cross-platform build & publish automation
- ✅ Added `.github/workflows/recorder-release.yml` with a platform matrix (manylinux x86_64/aarch64, macOS universal2, Windows amd64) and a Linux verification gate reusing our existing test suite.
- ✅ Integrated artefact collection plus a TestPyPI smoke install that exercises the CLI before invoking Trusted Publishing-friendly uploads.
- ✅ Added a guarded PyPI promotion job that reuses the staged artefacts and requires environment approval prior to publishing.

## Next Tasks
- First tagged release should monitor the new workflow end-to-end and capture any follow-up improvements in the release tracker issue.
