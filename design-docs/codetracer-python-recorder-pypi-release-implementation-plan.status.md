# `codetracer-python-recorder` PyPI Release – Status

## Workstream 1 – Package metadata & repository hygiene
- ✅ Tightened `pyproject.toml` metadata (Python 3.12–3.13 targets, classifiers, README long description, license file).
- ✅ Added maturin sdist include/exclude rules and a dedicated LICENSE copy for the crate.
- ✅ Introduced `scripts/check_recorder_version.py` and wired it into CI to enforce version parity.
- ✅ Documented supported environments in `codetracer-python-recorder/README.md` and added a release checklist under `design-docs/`.

## Next Tasks
- Begin Workstream 2: enhance build/test automation (extend Just recipes, add smoke install target, ensure integration coverage).
- Draft changes to the release workflow (`recorder-release.yml`) once Workstream 2 groundwork is ready.
