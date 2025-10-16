# ADR 0007: Input and Output Capture for Runtime Traces

- **Status:** Proposed
- **Date:** 2025-10-03
- **Deciders:** Runtime recorder maintainers
- **Consulted:** Python platform crew, Replay tooling crew
- **Informed:** DX crew, Release crew

## Context
- The repo now splits session bootstrap, monitoring glue, and runtime logic into clear modules (`session`, `monitoring`, `runtime`).
- `RuntimeTracer` owns the `NonStreamingTraceWriter` and activation rules, and already writes metadata, paths, and step events.
- `recorder-errors` gives us uniform error codes and panic trapping. Every new subsystem must use it.
- We still forward stdout, stderr, and stdin directly to the host console. No bytes reach the trace.
- Replay and debugging teams need IO events beside call and line records so they can rebuild console sessions.

## Problem
- We need lossless IO capture without breaking the in-process `sys.monitoring` design or the new error policy.
- The old pipe-based spec assumed the tracer lived inside `start()` and mutated global state freely. The refactor put lifecycle code behind `TraceSessionBootstrap`, `TraceOutputPaths`, and `RuntimeTracer::begin`.
- We also added activation gating and stricter teardown rules. Any IO hooks must respect them and always restore the original file descriptors.

## Decision
1. Keep the Python CLI contract. `codetracer_python_recorder.start_tracing` keeps installing the tracer, but now also starts an IO capture controller right before `install_tracer` and shuts it down inside `stop_tracing`.
2. Introduce `runtime::io_capture` with a single public type, `IoCapture`. It duplicates stdin/stdout/stderr, installs platform pipes, and spawns blocking reader threads. The module hides Unix vs Windows code paths behind a small trait (`IoEndpoint`).
3. Expose an `IoEventSink` from `RuntimeTracer`. The sink wraps the writer in `Arc<Mutex<...>>` and exposes two safe methods: `record_output(chunk: IoChunk)` and `record_input(chunk: IoChunk)`. Reader threads call the sink only. All conversions to `TraceLowLevelEvent` live next to the writer so we reuse value encoders and error helpers.
4. Extend `RuntimeTracer` with a light `ThreadSnapshotStore`. `on_line` updates the current `{ path_id, line, frame_id }` per Python thread. `IoEventSink` reads the latest snapshot when it serialises a chunk. When no snapshot exists we fall back to the last global step.
5. Store stdout and stderr bytes as `EventLogKind::Write` and `WriteOther`. Store stdin bytes as `EventLogKind::Read`. Metadata includes the stream name, monotonic timestamps, thread tag, and the captured snapshot when present. Bytes stay base64 encoded by the runtime tracing crate.
6. Keep console passthrough. The reader threads mirror each chunk back into the saved file descriptors so users still see live output.
7. Wire capture teardown into existing error handling. `IoCapture::stop` drains the pipes, restores FDs, signals the threads, and logs failures through the `recorder-errors` macros. `RuntimeTracer::finish` waits for the IO channel before calling `TraceWriter::finish_*` to avoid races.
8. Hide the feature behind `RecorderPolicy`. A new flag `policy.io_capture` defaults to off today. Tests and early adopters enable it. Once stable we flip the default.

## Consequences
- **Upsides:** We capture IO without a subprocess, reuse the refactored writer lifecycle, and keep activation gating intact. Replay tooling reads one stream for events and IO.
- **Costs:** Writer calls now cross a mutex, so we must measure contention. The new module adds platform code that needs tight tests. We must watch out for deadlocks on interpreter shutdown.

## Rollout
- Ship behind an environment toggle `CODETRACER_CAPTURE_IO=1` wired into the policy layer. Emit a warning when the policy disables capture.
- Document the behaviour in the recorder README and the user CLI help once we land the feature.
- Graduate the ADR to **Accepted** after the implementation plan closes and the policy ship flips the default on both Unix and Windows.

## Alternatives
- A subprocess wrapper was considered again and rejected. It would undo the refactor that keeps tracing in-process and would break existing embedding use cases.
- `sys.stdout` monkey patching remains off the table. It misses native writes and user-assigned streams.
- Writing IO into a separate JSON file is still unnecessary. The runtime tracing schema already handles IO events.
