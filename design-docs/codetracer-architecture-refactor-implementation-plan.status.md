# Codetracer Architecture Refactor – Status

## Task Summary
- **Objective:** Execute ADR 0011 by modularising `codetracer-python-recorder`, starting with Milestone 1 (Trace Filter Decomposition) to restore single-responsibility boundaries and reduce coupling.

## Relevant Design Docs
- `design-docs/adr/0011-codetracer-architecture-refactor.md`
- `design-docs/codetracer-architecture-refactor-implementation-plan.md`

## Key Source Files (Milestone 1 Focus)
- `codetracer-python-recorder/src/trace_filter/config.rs`
- `codetracer-python-recorder/src/trace_filter/engine.rs`
- `codetracer-python-recorder/src/session/bootstrap.rs`
- `codetracer-python-recorder/src/runtime/mod.rs`
- Associated `trace_filter` unit tests under `codetracer-python-recorder/src/trace_filter/`

## Progress Log
- ✅ Captured architectural intent in ADR 0011 and drafted the implementation plan with milestones and concept-to-file mapping.
- ✅ Logged this status tracker to maintain continuity across milestones.
- ✅ Milestone 1 Kickoff: catalogued existing `trace_filter` responsibilities and outlined target submodules (`model`, `loader`, `summary`, `engine` helpers).
  - `trace_filter/config.rs` audit:
    - **Model candidates:** `ExecDirective`, `ValueAction`, `IoStream`, `FilterMeta`, `IoConfig`, `ValuePattern`, `ScopeRule`, `FilterSource`, `FilterSummary`, `FilterSummaryEntry`, `TraceFilterConfig`.
    - **Loader utilities:** `ConfigAggregator` and helpers (`ingest_*`, `finish`, `calculate_sha256`, `detect_project_root`, `parse_meta`, `resolve_defaults`, `parse_*`, `parse_rules`, `parse_value_patterns`), plus private `Raw*` serde structs.
  - `trace_filter/engine.rs` audit:
    - **Model/shared:** `ExecDecision`, `ValueKind`, `ValuePolicy`, `ScopeResolution`.
    - **Engine core:** `TraceFilterEngine`, `CompiledScopeRule`, `CompiledValuePattern`, `ScopeContext`, compilation helpers (`compile_rules`, `compile_value_patterns`, `ScopeContext::derive`, `normalise_*`, `module_from_relative`, `py_attr_error`).
    - **Tests:** rely on helper `filter_with_pkg_rule`; will need relocation once modules split.
- ✅ Milestone 1 skeleton: added placeholder modules `trace_filter::model`, `::loader`, and `::summary`; updated `trace_filter::mod` to expose them while retaining existing `config`/`engine` facades for compatibility.
- ✅ Step 1 complete: relocated shared model types (`ExecDirective`, `ValueAction`, `IoStream`, `IoConfig`, `ValuePattern`, `ScopeRule`, `FilterSource`, `FilterMeta`, `FilterSummary*`, `TraceFilterConfig`) into `trace_filter::model`, re-exported them from `config`, and removed duplicate impls. `just test` verified the crate after the move.
- ✅ Step 2 complete: extracted loader utilities and serde `Raw*` structures into `trace_filter::loader`, rewrote the config facade to use `ConfigAggregator`, and rebuilt selector normalisation via `Selector::parse`. `just test` (Rust + Python suites) confirmed parsing works post-move.
- ✅ Step 3 complete: moved summary construction into `trace_filter::summary`, updated `TraceFilterConfig::summary` to delegate to the new helper, and re-ran `just test` (all Rust/Python tests pass).
- ✅ Facade review: `trace_filter::config` now re-exports model types and delegates to the loader; no redundant helpers remain. Module exports verified via `just test`.

- 🔄 Milestone 2 Kickoff: auditing `policy.rs` and `logging.rs` to classify responsibilities for modularisation.
  - `policy.rs` audit:
    - **Model candidates:** `OnRecorderError`, `IoCapturePolicy`, `RecorderPolicy`, `PolicyUpdate`, `PolicyPath`, `policy_snapshot`, POLICY cell helpers.
    - **Environment parsing:** constants (`ENV_*`), `configure_policy_from_env`, `parse_bool`, `parse_capture_io`.
    - **FFI bindings:** `configure_policy_py`, `py_configure_policy_from_env`, `py_policy_snapshot`, PyO3 imports/tests.
  - `logging.rs` audit:
    - **Logger core:** `RecorderLogger`, `FilterSpec`, init/apply helpers, destination management.
    - **Metrics:** `RecorderMetrics` trait, `NoopMetrics`, `install_metrics`, `metrics_sink`, telemetry recorders.
    - **Error trailers:** `emit_error_trailer`, trailer writer management.
    - **Shared utilities:** `with_error_code[_opt]`, `set_active_trace_id`, `log_recorder_error`, `JSON_ERRORS_ENABLED`.
- ✅ Milestone 2 scaffolding: created placeholder modules `policy::{model, env, ffi}` and `logging::{logger, metrics, trailer}`; top-level `policy.rs`/`logging.rs` still host existing logic pending extraction. `just test` validates the skeletal split compiles.
- ✅ Milestone 2 Step 1: moved policy data structures and global helpers into `policy::model`, re-exported public APIs, updated tests, and reran Rust/Python suites (`cargo nextest`, `pytest`) successfully.

### Planned Extraction Order (Milestone 2)
1. **Policy model split:** Move data structures (`OnRecorderError`, `IoCapturePolicy`, `RecorderPolicy`, `PolicyUpdate`, `PolicyPath`) and policy cell helpers (`policy_cell`, `policy_snapshot`, `apply_policy_update`) into `policy::model`. Expose minimal APIs for environment/FFI modules.
2. **Policy environment parsing:** Relocate `configure_policy_from_env`, env variable constants, and helper parsers (`parse_bool`, `parse_capture_io`) into `policy::env`, depending on `policy::model` for mutations.
3. **Policy FFI layer:** Migrate PyO3 functions (`configure_policy_py`, `py_configure_policy_from_env`, `py_policy_snapshot`) into `policy::ffi`, keeping tests alongside; ensure `lib.rs` uses the new module exports.
4. **Logging module split:** Extract `RecorderLogger`, `FilterSpec`, `init_rust_logging_with_default`, `apply_policy`, and log helpers into `logging::logger`. Place metrics trait/sink logic into `logging::metrics`, error trailer functions into `logging::trailer`, leaving `logging.rs` as the facade orchestrating shared utilities (`with_error_code`, `set_active_trace_id`).
5. **Update tests & imports:** Adjust unit tests to target new modules, ensure re-exports keep existing public API stable, and run `just test` after each stage.

### Planned Extraction Order (Milestone 1)
1. **Model types first:** Relocate shared enums/structs (`ExecDirective`, `ValueAction`, `IoStream`, `FilterMeta`, `IoConfig`, `ValuePattern`, `ScopeRule`, `FilterSource`, `FilterSummary*`, `TraceFilterConfig`) into `trace_filter::model`. Update `config.rs` to re-export or `use` the new module and adjust external call sites (`session/bootstrap.rs`, `runtime/mod.rs`, tests).
2. **Loader utilities next:** Port `ConfigAggregator`, parsing helpers (`ingest_*`, `calculate_sha256`, `detect_project_root`, `parse_*`, `parse_rules`, `parse_value_patterns`) and serde `Raw*` structs into `trace_filter::loader`. Provide a clean API (e.g., `Loader::finish() -> TraceFilterConfig`) consumed by the facade.
3. **Summary helpers:** Move filter summary construction into `trace_filter::summary`, ensuring metadata writers (`RuntimeTracer::append_filter_metadata`) switch to the new API.
4. **Facade cleanup:** Once pieces live in dedicated modules, shrink `config.rs` to a thin facade that orchestrates loader/model interactions and re-exports primary types. Keep backward-compatible function names for now.
5. **Tests:** After each move, update unit tests in `trace_filter` modules and dependent integration tests (`session/bootstrap.rs` tests, `runtime` tests). Targeted command: `just test` (covers Rust + Python suites).

## Next Actions
1. Begin Step 2: move environment parsing (env constants, `configure_policy_from_env`, `parse_bool`, `parse_capture_io`) into `policy::env`, adapting call sites.
2. After Step 2, relocate PyO3 bindings into `policy::ffi` and refresh exports before tackling logging splits.
