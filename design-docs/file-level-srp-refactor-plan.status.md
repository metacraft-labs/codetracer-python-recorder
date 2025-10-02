# File-Level SRP Refactor Status

## Current Status
- ✅ Step 2 complete: introduced `src/logging.rs` for one-time logger initialisation and migrated tracing session lifecycle (`start_tracing`, `stop_tracing`, `is_tracing`, `flush_tracing`, `ACTIVE` flag) into `src/session.rs`, with `src/lib.rs` now limited to PyO3 wiring and re-exports.
- ✅ Step 3 complete: added `src/runtime/mod.rs` with focused `activation`, `value_encoder`, and `output_paths` submodules; `RuntimeTracer` now delegates activation gating, value encoding, and writer initialisation through the façade consumed by `session.rs`.
- ⚠️ Test baseline still pending: `cargo check` succeeds; `cargo test` currently fails to link in the sandbox because CPython development symbols are unavailable, matching the pre-refactor limitation.

## Next Task
- Step 4: Modularise sys.monitoring glue by introducing `src/monitoring/mod.rs` and splitting trait/dispatcher logic as outlined in the plan.
