# ADR 0006: PyPI Release Strategy for `codetracer-python-recorder`

- **Status:** Proposed
- **Date:** 2025-10-13
- **Deciders:** Codetracer Runtime & Tooling Leads
- **Consulted:** Release Engineering, Developer Experience, Platform Reliability
- **Informed:** Product Management, Support, Security

## Context

We are ready to publish the first public release of the Rust-backed `codetracer-python-recorder`
module to PyPI. The business goal is to distribute an officially supported recording module for
Python 3.12 and 3.13 across Linux, macOS, and Windows, with both binary wheels and a source
distribution that downstream teams can audit and rebuild.

A repository review shows that:

- Packaging metadata in `codetracer-python-recorder/pyproject.toml` still targets Python 3.8+,
  omits Trove classifiers for supported versions and platforms, and does not expose project URLs
  or the README as the long description.
- The `Justfile` and CI only support developer builds (`maturin develop`) and Linux test jobs;
  there is no automated release workflow and no multi-platform wheel builds.
- `tool.maturin` configuration does not yet declare source-distribution rules, exclude build
  artefacts, or ensure that `Cargo.toml` / `pyproject.toml` versions stay synchronized.
- There is no documented path for staging releases on TestPyPI or for securely authenticating a CI
  workflow to PyPI.

We consulted the Python Packaging User Guide packaging flow
([packaging.python.org](https://packaging.python.org/en/latest/flow/)) and the maturin distribution
guide ([maturin.rs](https://www.maturin.rs/distribution.html)). Key takeaways include:

- Every release must produce both wheels and an sdist, validate metadata with tooling, and verify
  installability before uploading.
- PyPI now recommends OpenID Connect “Trusted Publishing” for CI-driven uploads instead of storing
  long-lived API tokens.
- maturin provides first-class support for building manylinux, macOS universal2, and Windows wheels,
  plus TestPyPI/PyPI uploads from CI runners.

## Decision

1. **Metadata hardening:** Update `pyproject.toml` to `requires-python = ">=3.12,<3.14"`, include
   the project README as the long description, add platform-specific Trove classifiers (Linux,
   macOS, Windows; CPython 3.12/3.13; Rust), and publish canonical URLs (homepage, repository,
   issues). Mirror the version in `Cargo.toml` and add a guard that compares Rust and Python
   versions during CI.
2. **Distribution artefacts:** Continue to use maturin as the build backend. For the initial public
   release, build per-version wheels (`cp312` and `cp313`) for
   - manylinux2014 `x86_64` and `aarch64`,
   - macOS universal2 (`x86_64` + `arm64`),
   - Windows `amd64`.
   Produce a source distribution via `maturin sdist`, ensuring `Cargo.lock`, Rust sources, and Python
   shim modules are included while excluding wheel artefacts (`target/`, compiled `.so` files).
   Evaluate enabling the `pyo3/abi3-py312` feature after the first release to collapse per-version
   wheels.
3. **Pre-release verification:** Extend the release pipeline to run unit tests against the built
   artefacts, execute smoke installs (`pip install` from the local wheel and sdist), and run the CLI
   (`python -m codetracer_python_recorder --help`) before any upload step.
4. **Trusted publishing workflow:** Create a dedicated GitHub Actions workflow triggered by
   annotated tags (e.g., `recorder-v*`). The workflow will:
   - Build and test wheels on each platform matrix job.
   - Upload artefacts to a staging job that performs TestPyPI publishing via maturin.
   - Require a manual approval (environment protection) before promoting the same artefacts to the
     production PyPI repository.
   Configure the PyPI project as a Trusted Publisher for the repository so uploads rely on OIDC
   tokens instead of stored secrets.
5. **Versioning & change management:** Adopt a documented version-bump process that updates both
   `pyproject.toml` and `Cargo.toml`, updates the changelog/release notes, and tags releases using
   `recorder-vMAJOR.MINOR.PATCH`. Enforce semantic-versioning semantics via review, and block
   accidental reuse of versions by asserting that the requested tag matches the metadata.
6. **Documentation & support:** Add a release checklist to the repository (under `design-docs`) that
   references PyPI’s packaging flow steps (metadata validation, TestPyPI verification, final
   release), and document how downstream consumers install the wheel (platform notes, supported
   Python versions).

## Alternatives Considered

- **Manual, developer-driven uploads:** Rejected because manual wheel builds are error-prone,
  difficult to reproduce across platforms, and conflict with PyPI’s recommendation to use trusted
  CI publishing.
- **cibuildwheel-backed workflow:** Considered but rejected for this release; maturin already owns
  the build backend, integrates with PyO3, and provides the required cross-platform coverage with
  less indirection.
- **Limiting support to Linux and source distributions:** Rejected because product requirements call
  for macOS and Windows parity from day one, and maturin’s cross-platform build support removes the
  main barrier to doing so.

## Consequences

- **Positive:** Reproducible, tested release artefacts; reduced risk of shipping mismatched metadata;
  streamlined release process that matches the official packaging flow; improved supply-chain
  posture through OIDC trusted publishing.
- **Negative:** Longer CI pipelines (multi-platform builds) and the operational overhead of
  configuring PyPI Trusted Publishing. macOS universal2 builds increase macOS runner usage, and
  cross-compiling for `aarch64` may extend Linux build times.
- **Risks & Mitigations:** maturin cross-build failures are possible—mitigate by caching Rust
  dependencies and adding smoke tests that catch platform-specific regressions early. If Trusted
  Publishing is unavailable during initial setup, fall back temporarily to a scoped PyPI API token
  stored in GitHub environments while tracking completion of the OIDC configuration.
