# `codetracer-python-recorder` Release Operations

This note supplements the release checklist with details about the automated workflow,
Trusted Publishing setup, and manual approvals required to promote artefacts from
TestPyPI to PyPI.

## GitHub Actions workflow

- Workflow file: `.github/workflows/recorder-release.yml`.
- Triggers:
  - `workflow_dispatch` – run a staged release without creating a tag. Supply the
    target ref (branch/tag) and monitor the jobs; artefacts are still produced but
    PyPI promotion is skipped unless the ref is a tagged release.
  - `push` to tags matching `recorder-v*`.
- Job sequence:
  1. `verify` (ubuntu) – prepares the dev environment via Nix, runs `just test`,
     and confirms Python/Rust version parity.
  2. `build` matrix (linux-x86_64, linux-aarch64, macos-universal2, windows-amd64) –
     uses maturin to build wheels (and the sdist on linux-x86_64) and uploads artefacts.
  3. `publish-testpypi` – aggregates artefacts, performs a Linux smoke install using
     `scripts/select_recorder_artifact.py`, then publishes to TestPyPI.
  4. `publish-pypi` – gated behind the protected environment. Runs only for tag pushes
     after manual approval, reusing artefacts from earlier jobs.

## Trusted Publishing configuration

- TestPyPI and PyPI both list `metacraft-labs/cpr-main` as a Trusted Publisher.
- The workflow requests OIDC tokens automatically; no API tokens are stored in secrets.
- GitHub environments:
  - `testpypi` – no approval required; used to track audit logs and enforce environment
    level variables if needed.
  - `pypi-production` – requires manual approval by a Release Engineer before the
    `publish-pypi` job starts. Approvers can approve directly from the workflow run UI.

## Maintainer checklist highlights

1. **Before dispatching the workflow**
   - Update version numbers in `pyproject.toml` and `Cargo.toml`.
   - Append a section to `codetracer-python-recorder/CHANGELOG.md`.
   - Ensure `python3 scripts/check_recorder_version.py` succeeds.
   - Optionally run `just smoke-wheel` locally.

2. **Running the workflow**
   - For dry runs (no tagging yet) use `workflow_dispatch` targeting the release branch.
   - For real releases, push the annotated tag (`recorder-vX.Y.Z`). The workflow validates
     the tag against the pyproject version.

3. **Approvals**
   - Watch the `publish-testpypi` job; ensure the smoke install step passes.
   - Approve the `pypi-production` environment to trigger promotion. Include a note in the
     approval dialog referencing the TestPyPI run result.

4. **Post-release**
   - Install from PyPI on Linux/macOS/Windows for Python 3.12 and 3.13
     (`pip install codetracer-python-recorder && python -m codetracer_python_recorder --help`).
   - Publish GitHub release notes that link back to the changelog entry.
   - Close the release tracker issue once validation is complete.

## Fallback procedure

If the workflow cannot obtain an OIDC token (e.g., PyPI incidents):

1. Temporarily disable the Trusted Publishing requirement in PyPI/TestPyPI.
2. Configure a scoped API token as an environment secret and rerun the failed publish job.
3. Re-enable Trusted Publishing immediately after resolving the incident.
4. Document the incident in the release tracker issue.
