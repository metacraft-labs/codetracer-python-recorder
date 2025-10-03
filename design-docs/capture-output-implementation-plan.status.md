# Capture Output Implementation Plan — Status

## Stage 0 – Refactor for IO capture
- **Status:** Completed (2025-10-03)
- **Highlights:** Introduced `TraceWriterHost`, added `ThreadSnapshotStore`, stubbed `IoDrain` hook, and taught `session::start_tracing` / `stop_tracing` to manage optional lifecycle handles. Added regression test `updates_thread_snapshot_store_for_active_thread` and ran `just test`.

## Stage 1 – Build the IO capture core
- **Status:** Completed (2025-10-03)
- **Highlights:** Introduced the `runtime::io_capture` module with Unix and Windows backends, wiring descriptor duplication, pipe installation, and crossbeam-channel queues into the new `IoCapture` worker harness. Added shutdown-safe mirroring of stdout/stderr plus stdin pumping, and covered the code with focused unit tests (`captures_stdout_and_preserves_passthrough`, `captures_stdin_and_forwards_bytes`, and a Windows tempfile-based regression). `just test` passes.

## Stage 2 – Connect capture to the tracer
- **Status:** Not started
- **Notes:** Depends on Stage 1 artifacts. Awaiting design sign-off for event metadata schema.

## Stage 3 – Policy flag, CLI wiring, and guards
- **Status:** Not started
- **Notes:** Pending output from Stages 1–2. Need decisions on CLI UX before implementation.

## Stage 4 – Hardening and docs
- **Status:** Not started
- **Notes:** Will schedule once capture experiments validate perf and stability.
