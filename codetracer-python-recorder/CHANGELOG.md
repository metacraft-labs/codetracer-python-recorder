# codetracer-python-recorder — Change Log

All notable changes to `codetracer-python-recorder` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
### Added
- Balanced call-stack handling for generators, coroutines, and unwinding frames by subscribing to `PY_YIELD`, `PY_UNWIND`, `PY_RESUME`, and `PY_THROW`, mapping resume/throw events to `TraceWriter::register_call`, yield/unwind to `register_return`, and capturing `PY_THROW` arguments as `exception` using the existing value encoder. Added Python + Rust integration tests that drive `.send()`/`.throw()` on coroutines and generators to guarantee the trace stays balanced and that exception payloads are recorded.

### Changed
- Module-level call events now prefer the frame's `__name__`, fall back to filter hints, `sys.path`, and package markers, and no longer depend on the legacy resolver/cache. The globals-derived naming flag now defaults to enabled so direct scripts record `<__main__>` while package imports emit `<pkg.mod>`, with CLI and environment overrides available for the legacy resolver.

## [0.2.0] - 2025-10-17
### Added
- Added configurable trace filters backed by layered TOML files with glob/regex/literal selectors for packages, files, objects, and value domains, strict schema validation via `TraceFilterConfig::from_paths`, and explicit `allow`/`redact`/`drop` value policies summarised with SHA-256 digests.
- Added `TraceFilterEngine` and runtime wiring that cache scope resolutions, gate tracing, substitute `<redacted>` for filtered payloads, drop suppressed variables entirely, and emit per-kind redaction/drop counters alongside filter summaries in `trace_metadata.json`.
- Exposed configurable filters through the Python API, auto-start hook, CLI (`--trace-filter`), and `CODETRACER_TRACE_FILTER` environment variable while always prepending the built-in default filter that skips stdlib noise and redacts common secrets before layering project overrides.
- Added filter-focused documentation and benchmarking coverage, including onboarding and README guides plus Criterion + Python smoke benchmarks orchestrated by `just bench`.
- Introduced a line-aware IO capture pipeline that records stdout/stderr chunks with `{path_id, line, frame_id}` attribution via the shared `LineSnapshotStore` and multi-threaded `IoEventSink`.
- Added `LineAwareStdout`, `LineAwareStderr`, and `LineAwareStdin` proxies that forward to the original streams while batching writes on newline, explicit `flush()`, 5 ms idle gaps, and step boundaries.
- Added policy, CLI, and environment toggles for IO capture (`--io-capture`, `configure_policy(io_capture_line_proxies=..., io_capture_fd_fallback=...)`, `CODETRACER_CAPTURE_IO`) alongside the `ScopedMuteIoCapture` guard that suppresses recursive recorder logging.
- Added an optional FD mirror fallback that duplicates `stdout`/`stderr`, diffs native writes against the proxy ledger, emits `mirror`-flagged `IoChunk`s, and restores descriptors on teardown.
- Documented IO capture behaviour in the README with ADR 0008 context, manual smoke instructions, and troubleshooting steps for replaced `sys.stdout` / `sys.stderr`.
- Documented the error-handling policy in the README, including the `RecorderError` hierarchy, policy hooks, JSON error trailers, exit codes, and sample handlers for structured failures.
- Added an onboarding guide at `docs/onboarding/error-handling.md` with migration steps for downstream tools.
- Added contributor guidance for assertions: prefer `bug!` / `ensure_internal!` over `panic!` / `.unwrap()`, and pair `debug_assert!` with classified errors.

## [0.1.0] - 2025-10-13
### Added
- Initial public release of the Rust-backed recorder with PyO3 bindings.
- Python façade (`codetracer_python_recorder`) exposing `start`, `stop`, `trace`, and the CLI entry point (`python -m codetracer_python_recorder`).
- Support for generating `trace_metadata.json` and `trace_paths.json` artefacts compatible with the Codetracer db-backend importer.
- Cross-platform packaging targeting CPython 3.12 and 3.13 on Linux (manylinux2014 `x86_64`/`aarch64`), macOS universal2, and Windows `amd64`.

[Unreleased]: https://github.com/metacraft-labs/cpr-main/compare/recorder-v0.2.0...HEAD
[0.2.0]: https://github.com/metacraft-labs/cpr-main/compare/recorder-v0.1.0...recorder-v0.2.0
[0.1.0]: https://github.com/metacraft-labs/cpr-main/releases/tag/recorder-v0.1.0
