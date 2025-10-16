# Configurable Trace Filters – Status

## Relevant Design Docs
- `design-docs/US0028 - Configurable Python trace filters.md`
- `design-docs/adr/0009-configurable-trace-filters.md`
- `design-docs/configurable-trace-filters-implementation-plan.md`

## Key Source Files
- `codetracer-python-recorder/src/trace_filter/selector.rs` *(new in WS1)*
- `codetracer-python-recorder/src/trace_filter/config.rs` *(new in WS2)*
- `codetracer-python-recorder/Cargo.toml`
- `codetracer-python-recorder/src/lib.rs`
- Future stages: `codetracer-python-recorder/src/trace_filter/engine.rs`, `codetracer-python-recorder/src/runtime/mod.rs`, Python surface files under `codetracer_python_recorder/`

## Stage Progress
- ✅ **WS1 – Selector Parsing & Compilation:** Added `globset`/`regex` dependencies and introduced `trace_filter::selector` with parsing logic, compiled matchers, and unit tests covering glob/regex/literal selectors plus validation errors. Verified via `just cargo-test` (nextest with `--no-default-features`) so we avoid CPython linking issues and exercise the new suite.
- ✅ **WS2 – Filter Model & Loader:** Added `trace_filter::config` with `TraceFilterConfig::from_paths`, strict schema validation, SHA256-backed `FilterSummary`, scope/value structs, and path normalisation for `file:` selectors. Dependencies `toml` and `sha2` wired via `Cargo.toml`. Unit tests cover composition, inheritance guards, unknown keys, IO validation, and literal path normalisation; exercised using `just cargo-test`.
- ⏳ **WS3 – Runtime Engine & Caching:** Pending WS2 outputs.
- ⏳ **WS4 – RuntimeTracer Integration:** Pending WS3.
- ⏳ **WS5 – Python Surface, CLI, Metadata:** Pending WS4.
- ⏳ **WS6 – Hardening, Benchmarks & Documentation:** Pending prior stages.

## Next Steps
1. Design WS3 `TraceFilterEngine` API: caching strategy, resolution struct, and how it will consume `TraceFilterConfig` defaults/rules.
2. Prototype module + object derivation helpers (module name, qualname) needed by WS3 so selectors operate on normalised module/object identifiers.
3. Extend test plan for WS3 to exercise caching and selector precedence; prepare scaffolding to mock `CodeObjectWrapper` inputs without activating CPython.
