# IO Capture Line-Proxy Plan – Status

## Relevant Design Docs
- `design-docs/adr/0008-line-aware-io-capture.md`
- `design-docs/io-capture-line-proxy-implementation-plan.md`
- Prototype reference: `design-docs/prototypes/io_capture_ledger_mirror_prototype.py`

## Key Source Files
- `codetracer-python-recorder/src/runtime/line_snapshots.rs`
- `codetracer-python-recorder/src/runtime/mod.rs`
- Runtime tests that exercise the snapshot lifecycle live in `codetracer-python-recorder/src/runtime/mod.rs`

## Stage Progress
- ✅ **Stage 0 – Prepare runtime hooks:** Added `LineSnapshotStore` with per-thread records (`path_id`, `line`, `frame_id`, timestamp), wired it into `RuntimeTracer::on_line`, exposed a read-only handle, and covered the store with unit plus integration tests. Cleanup paths clear the store on tracer finish.
- ⏳ **Stage 1 – Build IO proxy classes:** Not started.
- ⏳ **Stage 2 – Implement IoEventSink and batching:** Not started.
- ⏳ **Stage 3 – Wire proxies into lifecycle:** Not started.
- ⏳ **Stage 4 – Optional FD mirror:** Not started.
- ⏳ **Stage 5 – Hardening and docs:** Not started.

## Next Steps
1. Start Stage 1 by introducing the PyO3 proxy classes (`LineAwareStdout`, `LineAwareStderr`, `LineAwareStdin`) that forward to the original streams while capturing payloads.
2. Decide on the public surface (methods, safeguards) for the proxies and draft unit tests before broad integration.
3. Plan how Stage 1 changes will expose proxy events to the new snapshot store without tripping logging recursion.
