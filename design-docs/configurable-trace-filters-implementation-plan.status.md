# Configurable Trace Filters – Status

## Relevant Design Docs
- `design-docs/US0028 - Configurable Python trace filters.md`
- `design-docs/adr/0009-configurable-trace-filters.md`
- `design-docs/configurable-trace-filters-implementation-plan.md`

## Key Source Files
- `codetracer-python-recorder/src/trace_filter/selector.rs` *(new in WS1)*
- `codetracer-python-recorder/Cargo.toml`
- `codetracer-python-recorder/src/lib.rs`
- Future stages: `codetracer-python-recorder/src/trace_filter/{config.rs,engine.rs}`, `codetracer-python-recorder/src/runtime/mod.rs`, Python surface files under `codetracer_python_recorder/`

## Stage Progress
- ✅ **WS1 – Selector Parsing & Compilation:** Added `globset`/`regex` dependencies and introduced `trace_filter::selector` with parsing logic, compiled matchers, and unit tests covering glob/regex/literal selectors plus validation errors. Verified via `just cargo-test` (nextest with `--no-default-features`) so we avoid CPython linking issues and exercise the new suite.
- ⏳ **WS2 – Filter Model & Loader:** Pending WS1 completion.
- ⏳ **WS3 – Runtime Engine & Caching:** Pending WS2 outputs.
- ⏳ **WS4 – RuntimeTracer Integration:** Pending WS3.
- ⏳ **WS5 – Python Surface, CLI, Metadata:** Pending WS4.
- ⏳ **WS6 – Hardening, Benchmarks & Documentation:** Pending prior stages.

## Next Steps
1. Decide whether to pull the `toml` dependency forward into WS1 (to avoid churn when WS2 lands) or add it alongside the config loader.
2. Begin WS2 design sketch: outline the `TraceFilterConfig`/`ScopeRule` data model and error reporting approach so selector integration points are clear before coding.
3. Keep an eye on the PyO3 toolchain—`just cargo-test` uses `uv run` to supply Python 3.13; replicate that setup for future Rust test runs to sidestep linker errors we hit when invoking `cargo test` directly.
