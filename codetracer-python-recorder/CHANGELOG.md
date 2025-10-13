# Changelog

All notable changes to `codetracer-python-recorder` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2025-10-13

### Added
- Initial public release of the Rust-backed recorder with PyO3 bindings.
- Python façade (`codetracer_python_recorder`) exposing `start`, `stop`, `trace`,
  and CLI entry point (`python -m codetracer_python_recorder`).
- Support for generating `trace_metadata.json` and `trace_paths.json`
  artefacts compatible with the Codetracer db-backend importer.
- Cross-platform packaging definition targeting CPython 3.12 and 3.13 on
  Linux (manylinux2014 `x86_64`/`aarch64`), macOS universal2, and Windows `amd64`.

[0.1.0]: https://github.com/metacraft-labs/cpr-main/releases/tag/recorder-v0.1.0
