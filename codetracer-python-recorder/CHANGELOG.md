# codetracer-python-recorder — Change Log

All notable changes to `codetracer-python-recorder` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
### Added
- Documented the error-handling policy in the README, including the `RecorderError` hierarchy, policy hooks, JSON error trailers, exit codes, and sample handlers for structured failures.
- Added an onboarding guide at `docs/onboarding/error-handling.md` with migration steps for downstream tools.
- Added contributor guidance for assertions: prefer `bug!` / `ensure_internal!` over `panic!` / `.unwrap()`, and pair `debug_assert!` with classified errors.

## [0.1.0] - 2025-10-13
### Added
- Initial public release of the Rust-backed recorder with PyO3 bindings.
- Python façade (`codetracer_python_recorder`) exposing `start`, `stop`, `trace`, and the CLI entry point (`python -m codetracer_python_recorder`).
- Support for generating `trace_metadata.json` and `trace_paths.json` artefacts compatible with the Codetracer db-backend importer.
- Cross-platform packaging targeting CPython 3.12 and 3.13 on Linux (manylinux2014 `x86_64`/`aarch64`), macOS universal2, and Windows `amd64`.

[Unreleased]: https://github.com/metacraft-labs/cpr-main/compare/recorder-v0.1.0...HEAD
[0.1.0]: https://github.com/metacraft-labs/cpr-main/releases/tag/recorder-v0.1.0
