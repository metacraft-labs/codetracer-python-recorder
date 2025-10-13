# `codetracer-python-recorder` PyPI Release – Implementation Plan

This plan captures the work required to implement ADR 0006 (“PyPI Release Strategy for
`codetracer-python-recorder`”) and prepare the project for automated publishing.

---

## Workstream 1 – Package metadata & repository hygiene

1. **Tighten project metadata**
   - Update `codetracer-python-recorder/pyproject.toml` with `requires-python = ">=3.12,<3.14"`,
     `readme = "README.md"`, Trove classifiers for CPython 3.12/3.13 and the three target
     platforms, and `project.urls` entries (`Homepage`, `Repository`, `Issues`, `Changelog`).
   - Ensure licensing information references the repository MIT license file.
2. **Synchronize Rust/Python versions**
   - Add a repository script (e.g., `scripts/check_recorder_version.py`) that verifies the version in
     `pyproject.toml` matches `Cargo.toml`.
   - Wire the check into CI (pull requests and release workflow) so mismatches fail fast.
3. **Curate source-distribution contents**
   - Expand `[tool.maturin.sdist]` to include Rust sources, `Cargo.lock`, and Python shim modules
     while excluding build artefacts (`target/`, cached wheels, compiled `.so` files).
   - Add a `.gitignore` entry for `codetracer_python_recorder/*.so` if not already covered.
4. **Document supported environments**
   - Update `codetracer-python-recorder/README.md` with explicit notes on supported Python versions,
     OS targets, and a short install section showing `pip install codetracer-python-recorder`.
   - Add a release checklist under `design-docs/` describing version bumps, tagging, and the TestPyPI
     verification gate.

## Workstream 2 – Build & test enhancements

1. **Augment local build tooling**
   - Extend `Justfile` targets (`build`, `build-all`) to invoke `maturin build --release --sdist`
     for the configured Python interpreters (3.12, 3.13) and to drop artefacts under
     `codetracer-python-recorder/target`.
   - Provide a `just smoke-wheel` recipe that creates a virtual environment, installs the freshly
     built wheel/sdist, and runs `python -m codetracer_python_recorder --help`.
2. **Strengthen automated tests**
   - Ensure existing Rust `cargo nextest` and Python `pytest` suites run as part of every release
     build.
   - Add an integration smoke test that exercises `start_tracing` and `stop_tracing` through the
     Python facade with temporary directories to guard against obvious regressions.
3. **Cache dependencies**
   - Configure `maturin` to use `--locked` builds and enable Rust/Python dependency caching (UV,
     Cargo) in CI to keep build times predictable across platforms.

## Workstream 3 – Cross-platform build & publish automation

1. **Create release workflow**
   - Add `.github/workflows/recorder-release.yml` triggered by annotated tags (`recorder-v*`) and a
     manual `workflow_dispatch`.
   - Define a matrix for:
     - Linux: `ubuntu-latest` using `messense/maturin-action` to build manylinux2014 `x86_64` and
       `aarch64` wheels.
     - macOS: `macos-13` to build universal2 wheels via `maturin build --universal2`.
     - Windows: `windows-latest` to build `win_amd64` wheels.
   - Run `just test` (or explicit Rust/Python test commands) before building artefacts on every job.
2. **Stage artefacts**
   - Upload all wheels and the sdist as GitHub Actions artefacts.
   - Add a downstream job that collects the artefacts, performs `pip install` smoke tests on Linux,
     and publishes to TestPyPI using `maturin upload --repository testpypi`.
3. **Trusted publishing setup**
   - Register the GitHub repository as a PyPI Trusted Publisher, mapping the release workflow to the
     PyPI project.
   - Configure GitHub environments to require manual approval (“Promote to PyPI”) and run
     `maturin upload --repository pypi` only after TestPyPI verification succeeds.
   - Document fallback instructions for using scoped API tokens if Trusted Publishing cannot be
     completed before the first release.

## Workstream 4 – Operational readiness & documentation

1. **Release management**
   - Prepare a CHANGELOG entry for v0.1.0 (or the first publicly published version) following
     Conventional Commits.
   - Document how to create and push annotated tags (`git tag -a recorder-vX.Y.Z`).
2. **Post-publish verification**
   - Describe the post-release validation steps (download from PyPI on each platform, run CLI smoke
     tests, monitor PyPI stats).
   - Create an issue template for future release tracking that references the checklist.

---

**Exit criteria:** The new release workflow successfully publishes a TestPyPI build from CI, a human
approves promotion to PyPI via the protected environment, and the published package installs and
passes smoke tests on Linux, macOS, and Windows for Python 3.12 and 3.13.
