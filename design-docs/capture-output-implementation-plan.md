# Capture Output Implementation Plan

## Goal
- Ship lossless stdout, stderr, and stdin capture in the Rust recorder without breaking the current CLI flow or error policy.

## Guiding Notes
- Follow ADR 0007.
- Keep sentences short for readers; prefer bullets.
- Run `just test` on every stage.

## Stage 0 – Refactor for IO capture (must land first)
- Split writer ownership out of `RuntimeTracer` into a helper (`TraceWriterHost`) that exposes a thread-safe event API.
- Add a small `ThreadSnapshotStore` that records the latest `{path_id, line, frame_id}` per Python thread inside the runtime module.
- Ensure `RuntimeTracer::finish` already waits on background work hooks; add a stub `IoDrain` trait with no-op implementation so later stages can slot in real drains.
- Update `session::start_tracing` and `stop_tracing` to accept optional "extra lifecycle" handles so we can pair start/stop work without more globals.
- Tests: extend existing runtime unit tests to cover the new snapshot store and confirm start/stop paths still finalise trace files.

## Stage 1 – Build the IO capture core
- Create `runtime::io_capture` with platform-specific back ends (`unix.rs`, `windows.rs`) hidden behind a common trait.
- Implement descriptor/handle duplication, pipe install, and reader thread startup. Use blocking reads and thread-safe queues (`crossbeam-channel` already in workspace; add if missing).
- Ensure mirror writes go back to the saved descriptors so console output stays live.
- Tests: add Rust unit tests that fake pipes (use `os_pipe` on Unix, `tempfile` handles on Windows via CI) to confirm duplication and restoration.

## Stage 2 – Connect capture to the tracer
- Add an `IoEventSink` struct that owns `Arc<Mutex<TraceWriterHost>>` plus a snapshot reader.
- Reader threads push `IoChunk` structs (`stream`, `timestamp`, `bytes`, `producer_thread`) into the sink. The sink converts them into runtime tracing events and records them.
- Use `recorder-errors` for all failures (`usage!` for bad config, `enverr!` for IO problems). Log through the existing logging module; never `println!`.
- Update `RuntimeTracer::begin` to start the sink when policy allows. Store the `IoCapture` handle and drain it in `finish`.
- Tests: add integration tests in `tests/` that run a sample script writing to stdout/stderr and reading from stdin, then assert trace files contain the matching events. Verify passthrough stays intact.

## Stage 3 – Policy flag, CLI wiring, and guards
- Extend `RecorderPolicy` with `io_capture_enabled` plus env var `CODETRACER_CAPTURE_IO`.
- Make the Python CLI surface a `--capture-io` flag (defaults to policy). Document the flag in help text.
- Emit a single log line when capture is disabled by policy so users understand why their trace lacks IO events.
- Tests: Python integration test toggling the policy and checking presence/absence of IO records.

## Stage 4 – Hardening and docs
- Stress test with large outputs (beyond pipe buffer) and interleaved writes from multiple threads.
- Run Windows CI to verify handle restore logic and CRLF behaviour.
- Document the feature in README + design docs. Update ADR status once accepted.
- Add metrics for dropped IO chunks using the existing logging counters.
- Tests: extend stress tests plus regression tests for start/stop loops to ensure descriptors always restore.

## Milestones
1. Stage 0 merged and green CI. Serves as base branch for feature work.
2. Stages 1–2 merged together behind a feature flag. Feature hidden by default.
3. Stage 3 flips the flag for opted-in users. Gather feedback.
4. Stage 4 finishes docs, flips default to on, and promotes ADR 0007 to Accepted.

## Verification Checklist
- `just test` passes after every stage.
- New unit tests cover writer host, snapshot store, and IO capture workers.
- Integration tests assert trace events and passthrough behaviour on Linux and Windows.
- Manual smoke: run `python -m codetracer_python_recorder examples/stdout_script.py` and confirm console output plus IO trace entries.

## Risks & Mitigations
- **Deadlocks:** Keep reader threads simple, use bounded channels, and add shutdown timeouts tested in CI.
- **Performance hit:** Benchmark before and after Stage 2 with large stdout workloads; document results.
- **Platform drift:** Share the Unix/Windows API contract in a `README` inside the module and guard behaviour with tests.

## Exit Criteria
- IO events present in trace files when the policy flag is on.
- Console output unchanged for users.
- No file descriptor leaks (checked via stress tests and `lsof` in CI scripts).
- Documentation published and linked from ADR 0007.
