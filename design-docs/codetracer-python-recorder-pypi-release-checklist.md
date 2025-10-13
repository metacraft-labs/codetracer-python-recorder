# `codetracer-python-recorder` PyPI Release Checklist

1. **Plan the release**
   - Confirm target version and ensure the changelog/release notes cover all user-visible changes.
   - Review open issues for blockers and verify CI is green on `main`.
2. **Update metadata**
   - Bump the version in both `codetracer-python-recorder/pyproject.toml` and `codetracer-python-recorder/Cargo.toml`.
   - Run `python scripts/check_recorder_version.py` to confirm parity.
   - Regenerate or update documentation if required (`README.md`, API docs).
3. **Validate locally**
   - Execute `just venv 3.12 dev` (or preferred interpreter) and run `just test`.
   - Build wheels and sdist (`just build` or `maturin build --release --sdist`) and perform a smoke install in a clean virtualenv (`python -m pip install dist/*.whl` and run `python -m codetracer_python_recorder --help`).
4. **Prepare the release tag**
   - Commit changes following Conventional Commits.
   - Tag the release with `git tag -a recorder-vX.Y.Z -m "codetracer-python-recorder vX.Y.Z"` and push (`git push origin recorder-vX.Y.Z`).
5. **Trigger CI publishing**
   - Monitor the `recorder-release` workflow.
   - Verify that TestPyPI publishing succeeds and smoke tests pass on all platforms.
6. **Promote to PyPI**
   - Approve the protected “Promote to PyPI” environment to publish the previously built artefacts.
   - Confirm the workflow completes without errors.
7. **Post-release tasks**
   - Validate installation directly from PyPI on Linux, macOS, and Windows for Python 3.12 and 3.13.
   - Publish release notes (GitHub Release) and notify stakeholders.
   - Create follow-up issues for any tasks deferred from the release.

Refer to the Python Packaging User Guide packaging flow (<https://packaging.python.org/en/latest/flow/>)
and the maturin distribution guide (<https://www.maturin.rs/distribution.html>) for further detail on
packaging expectations.
