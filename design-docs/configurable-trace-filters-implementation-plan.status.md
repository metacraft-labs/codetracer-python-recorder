# Configurable Trace Filters – Status

## Relevant Design Docs
- `design-docs/US0028 - Configurable Python trace filters.md`
- `design-docs/adr/0009-configurable-trace-filters.md`
- `design-docs/configurable-trace-filters-implementation-plan.md`

## Key Source Files
- `codetracer-python-recorder/src/trace_filter/selector.rs` *(new in WS1)*
- `codetracer-python-recorder/src/trace_filter/config.rs` *(new in WS2)*
- `codetracer-python-recorder/src/trace_filter/engine.rs` *(new in WS3)*
- `codetracer-python-recorder/src/session/bootstrap.rs` *(updated in WS4)*
- `codetracer-python-recorder/src/session.rs` *(updated in WS4)*
- `codetracer-python-recorder/codetracer_python_recorder/session.py` *(WS5 python API wiring)*
- `codetracer-python-recorder/codetracer_python_recorder/cli.py` *(WS5 CLI plumbing)*
- `codetracer-python-recorder/codetracer_python_recorder/auto_start.py` *(WS5 env integration)*
- `codetracer-python-recorder/tests/python/unit/test_auto_start.py` *(WS5 env regression coverage)*
- `codetracer-python-recorder/tests/python/unit/test_session_helpers.py`
- `codetracer-python-recorder/tests/python/unit/test_cli.py`
- `codetracer-python-recorder/Cargo.toml`
- `codetracer-python-recorder/src/lib.rs`
- Future stages: `codetracer-python-recorder/src/runtime/mod.rs`, Python surface files under `codetracer_python_recorder/`

## Stage Progress
- ✅ **WS1 – Selector Parsing & Compilation:** Added `globset`/`regex` dependencies and introduced `trace_filter::selector` with parsing logic, compiled matchers, and unit tests covering glob/regex/literal selectors plus validation errors. Verified via `just cargo-test` (nextest with `--no-default-features`) so we avoid CPython linking issues and exercise the new suite.
- ✅ **WS2 – Filter Model & Loader:** Added `trace_filter::config` with `TraceFilterConfig::from_paths`, strict schema validation, SHA256-backed `FilterSummary`, scope/value structs, and path normalisation for `file:` selectors. Dependencies `toml` and `sha2` wired via `Cargo.toml`. Unit tests cover composition, inheritance guards, unknown keys, IO validation, and literal path normalisation; exercised using `just cargo-test`.
- ✅ **WS3 – Runtime Engine & Caching:** Implemented `trace_filter::engine` with `TraceFilterEngine::resolve` caching `ScopeResolution` entries per code id (DashMap), deriving module/object/file metadata, and compiling value policies with ordered pattern evaluation. Added `ValueKind` to align future runtime integration and unit tests proving caching, rule precedence (object > package/file), and relative path normalisation—all exercised via `just cargo-test`.
- ✅ **WS4 – RuntimeTracer Integration:** `RuntimeTracer` now accepts an optional `Arc<TraceFilterEngine>`, caches `ScopeResolution` results per code id, and records `filter_scope_skip` when scopes are denied. Value capture helpers honour `ValuePolicy` with a reusable `<redacted>` sentinel, emit per-kind telemetry, and we persist the active filter summary plus skip/redaction counts into `trace_metadata.json`. Bootstrapping now discovers `.codetracer/trace-filter.toml`, instantiates `TraceFilterEngine`, and passes the shared `Arc` into `RuntimeTracer::new`; new `session::bootstrap` tests cover both presence/absence of the default filter and `just cargo-test` (nextest `--no-default-features`) confirms the flow end-to-end.
- ✅ **WS5 – Python Surface, CLI, Metadata:** Session helpers normalise chained specs, auto-start honours `CODETRACER_TRACE_FILTER`, PyO3 merges explicit/default chains, CLI exposes `--trace-filter`, unit coverage exercises env auto-start filter chaining, and docs/CLI help now describe filter precedence and env wiring.
- ⏳ **WS6 – Hardening, Benchmarks & Documentation:** Pending prior stages.

## WS5 Progress Checklist
1. ✅ Introduced Python-side helpers that normalise `trace_filter` inputs (strings, Paths, iterables) into absolute path chains, updated session API/context manager, and threaded env-driven auto-start.
2. ✅ Extended the PyO3 surface (`start_tracing`) and bootstrap loader to merge explicit specs with discovered defaults before building a shared `TraceFilterEngine`.
3. ✅ Updated CLI/env plumbing (`--trace-filter`, `CODETRACER_TRACE_FILTER`) plus unit/integration coverage exercising CLI parsing and end-to-end filter metadata.

## Next Steps
1. ✅ Added explicit tests for `CODETRACER_TRACE_FILTER` auto-start behaviour so env-driven sessions verify filter chaining (`codetracer_python_recorder/tests/python/unit/test_auto_start.py`).
2. ✅ Drafted docs/CLI help updates covering the new flag, env var, and filter discovery precedence ahead of WS5 sign-off.
3. Plan integration of `TraceFilterConfig::io` toggles with runtime IO capture enablement before moving into WS6.
