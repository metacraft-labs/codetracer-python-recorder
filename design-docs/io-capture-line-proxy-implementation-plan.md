# Line-Aware IO Capture Implementation Plan

This plan replaces the old pipe-based capture plan. Sentences stay short for easy reading. Each stage ends with tests and exit checks.

## Stage 0 – Prepare runtime hooks
- Add a `LineSnapshotStore` in the runtime crate if it does not already exist. It records `{path_id, line, frame_id, timestamp}` per thread.
- Expose a read-only view of the snapshot store so other modules can fetch the latest entry using a thread id.
- Extend `RuntimeTracer` to keep a handle to the snapshot store. Ensure the handle works from monitoring callbacks and from the IO layer.
- Tests: unit tests for snapshot updates, eviction on thread exit, and concurrent read/write.
- Exit: recorder still passes `just test`, tracing lifecycle unaffected.

## Stage 1 – Build IO proxy classes
- Create `runtime::io_lines` module with the PyO3 proxy structs (`LineAwareStdout`, `LineAwareStderr`, `LineAwareStdin`).
- Each proxy holds the original stream, a weak pointer to the shared `IoEventSink`, and a reentrancy guard.
- Implement the Python surface: `write`, `writelines`, `flush` for output; `read`, `readline`, `readinto`, `__iter__`, `__next__` for input. Methods delegate to the original stream.
- Capture payloads before returning. Store them together with the current thread id.
- Tests: Rust unit tests using `Python::with_gil` to ensure proxies mirror data, flush behaviour, and error handling. Python-level tests that monkeypatch `sys.stdout` and verify `print` still works.
- Exit: No leaks when we install/uninstall proxies repeatedly inside tests.

## Stage 2 – Implement IoEventSink and batching
- Add `IoChunk` struct holding `{stream, payload, thread_id, snapshot, timestamp, flags}`.
- `IoEventSink` groups writes by thread and stream. Batches flush when we hit newline, explicit flush, a Step boundary, or a 5 ms timer. Use `parking_lot::Mutex` and a `once_cell::sync::Lazy` timer wheel to keep locking simple.
- Provide `flush_before_step(thread_id)` API. The monitoring callbacks call it right before they emit a Step event, then record the Step, then update the snapshot store. This enforces the `Step -> IO -> next Step` ordering.
- Convert chunks into runtime trace events right after batching. Reuse existing encoders.
- Integrate with the recorder error macros for faults (`usage!`, `ioerr!`).
- Tests: unit tests for batching rules, timer flush, newline handling, guard on recursion, and the Step ordering API.
- Exit: sink drops zero events during stress tests that flood stdout with short writes.

## Stage 3 – Wire proxies into lifecycle
- Extend policy with `io_capture` options plus env parsing (`CODETRACER_CAPTURE_IO`).
- `start_tracing` installs proxies when policy allows. Also expose a Python helper `configure_policy_py` update to set the flags.
- `stop_tracing` tears down proxies even if tracing never started. Hook teardown into panic/unwind paths.
- Add `ScopedMuteIoCapture` and use it in logging and error reporting code.
- Tests: integration test launching a script that prints from Python and a C extension (use `ctypes` calling `PySys_WriteStdout`). Ensure events carry file path and line numbers.
- Exit: recorded IO events align with line events in the trace format.

## Stage 4 – Optional FD mirror
- Implement `FdMirror` that duplicates `stdout`/`stderr` descriptors and reads unseen bytes through a background thread.
- Maintain a per-stream ledger: a FIFO byte buffer plus the next sequence id. Proxy callbacks append `{sequence, bytes}` entries while holding a short lock. The sequence uses an atomic counter so we can spot skipped entries during debugging.
- Each mirror read walks a streaming diff: scan the chunk left-to-right, skip unmatched native bytes, and peel ledger entries whenever their bytes appear (even when native writes come first). Handle partial matches at chunk boundaries so long entries carry across reads. Whatever bytes remain become the mirror-only payload. This keeps capture lossless even when native writers interleave unexpectedly.
- Document why ordering holds in the common case: CPython keeps the GIL during `write`, so proxy order matches FD write order. Native writers run once the proxy releases the GIL, so their bytes appear after any proxy prefixes.
- The reader tags leftover events as `source = FdMirror` and uses the latest snapshot per thread for attribution.
- Tie the mirror to `policy.io_capture.fd_fallback`. Default off. Skip on platforms where dup is unsupported.
- Tests: stress test with `os.write(1, b"...")`, mixed proxy/mirror writes, mismatched sequences, and multiple Python threads. Ensure desync resets leave no duplicates and teardown restores descriptors.
- Exit: mirror can be toggled at runtime without breaking the proxies.

## Stage 5 – Hardening and docs
- Add timeouts to teardown so we avoid deadlocks when threads hang.
- Document the feature in the README and link to ADR 0008.
- Provide troubleshooting notes (how to detect when another tool replaces `sys.stdout`).
- Run cross-platform CI (Linux + Windows). Manual smoke: run `python -m codetracer_python_recorder examples/stdout_script.py`.
- Exit: telemetry shows zero dropped chunks; docs merged; ADR status ready to flip to Accepted.

## Verification Checklist
- `just test` passes after each stage.
- New unit and integration tests cover proxies, batching, policy toggles, and FD mirror.
- Manual smoke checks confirm console output stays live and line references match expectations.
- Logging guard stops recursive capture of recorder logs.
