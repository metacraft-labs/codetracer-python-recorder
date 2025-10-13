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

## Next Tasks
- Kick off Workstream 3: design the cross-platform release workflow (`recorder-release.yml`) and wire in TestPyPI publishing.
- Add release-pipeline steps to invoke the new smoke install target once the workflow skeleton exists.
