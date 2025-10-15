# IO Capture Line-Proxy Plan – Status

## Relevant Design Docs
- `design-docs/adr/0008-line-aware-io-capture.md`
- `design-docs/io-capture-line-proxy-implementation-plan.md`
- Prototype reference: `design-docs/prototypes/io_capture_ledger_mirror_prototype.py`

## Key Source Files
- `codetracer-python-recorder/src/runtime/line_snapshots.rs`
- `codetracer-python-recorder/src/runtime/mod.rs`
- `codetracer-python-recorder/src/runtime/io_lines.rs`
- Runtime tests that exercise the snapshot lifecycle live in `codetracer-python-recorder/src/runtime/mod.rs`

## Stage Progress
- ✅ **Stage 0 – Prepare runtime hooks:** Added `LineSnapshotStore` with per-thread records (`path_id`, `line`, `frame_id`, timestamp), wired it into `RuntimeTracer::on_line`, exposed a read-only handle, and covered the store with unit plus integration tests. Cleanup paths clear the store on tracer finish.
- ✅ **Stage 1 – Build IO proxy classes:** Brought `runtime::io_lines` back into the build, ported the proxy implementations to the PyO3 0.25 Bound/IntoPyObject APIs, and restored the unit tests that verify stdout/stderr passthrough, stdin reads, and the reentrancy guard. `just test` now exercises the proxies end-to-end.
- ⏳ **Stage 2 – Implement IoEventSink and batching:** Not started.
- ⏳ **Stage 3 – Wire proxies into lifecycle:** Not started.
- ⏳ **Stage 4 – Optional FD mirror:** Not started.
- ⏳ **Stage 5 – Hardening and docs:** Not started.

## Next Steps
1. Design `IoEventSink` batching per Stage 2: define `IoChunk`, per-thread buffers, and newline/flush/time-based emission rules.
2. Expose `flush_before_step(thread_id)` and integrate it with the monitoring callbacks so the batching layer can align with Step events.
3. Update the proxies to emit into the new sink implementation and backfill unit tests that cover batching boundaries and thread isolation.
