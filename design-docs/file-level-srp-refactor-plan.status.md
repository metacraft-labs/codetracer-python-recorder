# File-Level SRP Status Snapshot

## What’s done
- ✅ Logging + session split: `src/logging.rs` handles init, `src/session.rs` owns `start/stop/is_tracing` while `lib.rs` just wires PyO3.
- ✅ Runtime breakup: `RuntimeTracer` now lives in `runtime/mod.rs` with dedicated `activation`, `value_encoder`, and `output_paths` modules.
- ✅ Monitoring split: shared types live in `monitoring/mod.rs`; the trait + dispatcher sit in `monitoring/tracer.rs`; public re-exports stayed stable.
- ✅ Python package tidy-up: `formats.py`, `session.py`, and `auto_start.py` carry their own concerns; `api.py` is a thin façade and `__init__.py` runs the optional auto-start.
- ✅ Cleanup: removed TODOs, refreshed docs, and `just test` (nextest + pytest) passes with the repo-local UV cache.

## What’s next
- Nothing active—open new tasks if new requirements appear.
