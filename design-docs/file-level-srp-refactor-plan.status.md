# File-Level SRP Refactor Status

## Current Status
- ✅ Step 2 complete: introduced `src/logging.rs` for one-time logger initialisation and migrated tracing session lifecycle (`start_tracing`, `stop_tracing`, `is_tracing`, `flush_tracing`, `ACTIVE` flag) into `src/session.rs`, with `src/lib.rs` now limited to PyO3 wiring and re-exports.
- ✅ Step 3 complete: added `src/runtime/mod.rs` with focused `activation`, `value_encoder`, and `output_paths` submodules; `RuntimeTracer` now delegates activation gating, value encoding, and writer initialisation through the façade consumed by `session.rs`.
- ✅ Step 4 complete: introduced `src/monitoring/mod.rs` for sys.monitoring types/caches and `src/monitoring/tracer.rs` for the tracer trait plus callback dispatch; rewired `lib.rs`, `session.rs`, and `runtime/mod.rs`, and kept a top-level `tracer` re-export for API stability.
- ✅ Test baseline: `just test` (nextest + pytest) passes with the UV cache scoped to the workspace; direct `cargo test` still requires CPython development symbols.

## Next Task
- Step 5: Decompose the Python helper package into `session.py`, `formats.py`, and `auto_start.py`, updating `api.py`/`__init__.py` accordingly.
