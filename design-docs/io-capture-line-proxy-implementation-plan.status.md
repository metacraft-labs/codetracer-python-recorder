# IO Capture Line-Proxy Plan – Status

## Relevant Design Docs
- `design-docs/adr/0008-line-aware-io-capture.md`
- `design-docs/io-capture-line-proxy-implementation-plan.md`
- Prototype reference: `design-docs/prototypes/io_capture_ledger_mirror_prototype.py`

## Key Source Files
- `codetracer-python-recorder/src/runtime/line_snapshots.rs`
- `codetracer-python-recorder/src/runtime/mod.rs`
- `codetracer-python-recorder/src/runtime/io_capture/`
- Runtime tests that exercise the snapshot lifecycle live in `codetracer-python-recorder/src/runtime/mod.rs`

## Stage Progress
- ✅ **Stage 0 – Prepare runtime hooks:** Added `LineSnapshotStore` with per-thread records (`path_id`, `line`, `frame_id`, timestamp), wired it into `RuntimeTracer::on_line`, exposed a read-only handle, and covered the store with unit plus integration tests. Cleanup paths clear the store on tracer finish.
- ✅ **Stage 1 – Build IO proxy classes:** Brought `runtime::io_capture::proxies` into the build, ported the proxy implementations to the PyO3 0.25 Bound/IntoPyObject APIs, and restored the unit tests that verify stdout/stderr passthrough, stdin reads, and the reentrancy guard. `just test` now exercises the proxies end-to-end.
- ✅ **Stage 2 – Implement IoEventSink and batching:** Added the `IoChunk` model plus a `IoEventSink` batching layer that groups stdout/stderr writes per thread, flushes on newline, explicit `flush()`, step boundaries, and 5 ms gaps, and emits stdin reads immediately. Updated the proxies to surface flush events and introduced focused unit tests that cover batching, timer splits, step flushes, and stdin capture. `just test` runs the sink tests alongside the existing proxy coverage.
- ✅ **Stage 3 – Wire proxies into lifecycle:** `RuntimeTracer::install_io_capture` now instantiates the sink, installs the proxies behind the policy flag, and drains/flushed buffered chunks at step and finish boundaries. `IoChunk` records path IDs, frame IDs, and thread IDs sourced from the `LineSnapshotStore`, with a Python stack fallback filling metadata when monitoring snapshots are not yet available. Metadata emitted by `RecordEvent` now includes `path_id`, `line`, and `frame_id` for stdout/stderr chunks, and the Stage 3 integration test passes end-to-end.
- 🔄 **Stage 4 – Optional FD mirror:** Implemented the shared ledger (`runtime::io_capture::fd_mirror`), plumbed optional `MirrorLedgers`/`FdMirrorController` through `IoEventSink` and `RuntimeTracer::install_io_capture`, and added runtime tests that assert `os.write` payloads are captured only when `io_capture_fd_fallback` is enabled. Next actions: tighten metadata/telemetry (expose mirror stats, warn when descriptor duplication fails) and stress-test concurrent native writers.
- ⏳ **Stage 5 – Hardening and docs:** Not started.

## Next Steps
1. Stage 4: finish FD mirror wiring — add mirror-only tests (`os.write` / mixed stdout/stderr), surface user-facing warnings on setup failure, and document the new `mirror` flag in chunk metadata.
2. Stage 5 hardening: tighten logging guards (`ScopedMuteIoCapture`), expand integration coverage (policy disabled paths, multi-threaded writers), and document configuration knobs for IO capture.
3. Evaluate performance impact of the Python stack fallback and gate it behind monitoring snapshots once `sys.monitoring` integration fully drives the snapshot store.
