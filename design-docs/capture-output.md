# ADR: Non-invasive stdout/stderr/stdin capture and line-level mapping for the Python tracer (PyO3 + runtime_tracing)

**Status**: Accepted
**Date**: 2025-10-01
**Owners**: Tracing/Recorder team
**Scope**: Recorder runtime (Rust/PyO3) and Python instrumentation glue

---

## Context

- The user-facing CLI remains `python -m codetracer_python_recorder`.
- On startup the CLI parses recorder flags, adjusts `sys.argv`, and calls `codetracer_python_recorder.start(...)`, which delegates to the Rust extension.
- The Rust side already installs a `RuntimeTracer` implementation that subscribes to `sys.monitoring` callbacks and writes runtime_tracing artifacts (`trace.bin`/`trace.json`, `trace_metadata.json`, `trace_paths.json`).
- We still lack non-invasive capture of stdout/stderr/stdin that aligns each chunk with the trace stream for replay.
- The original draft assumed the script would be executed from Rust; in practice `runpy.run_path` stays inside the Python entrypoint, so the design must integrate with that lifecycle.
- The runtime_tracing crate (`TraceWriter`, `TraceLowLevelEvent::Event`, `EventLogKind::*`) already provides primitives for persisting arbitrary IO events; we will rely on it rather than introducing a bespoke artifact.

---

## Decision

1. **Lifecycle & CLI compatibility**  
   Retain the existing Python launcher. `python -m codetracer_python_recorder script.py` continues to prepare `sys.argv`, call `start(...)`, execute `runpy.run_path`, and finally `stop()`. Capture plumbing lives entirely inside `start()`/`stop()`, keeping the public API stable.

2. **FD-level capture component**  
   Within `start_tracing` we instantiate an `OutputCapture` controller that duplicates the original stdin/stdout/stderr descriptors (or Windows handles), installs pipes in their place, and spawns draining threads. The Python script still runs in-process, but every write to fd 1/2 is diverted through the controller. Each chunk receives a monotonic timestamp before being queued, and the bytes are simultaneously mirrored back to the preserved descriptors so users continue to see live console output.

3. **stdin strategy**  
   The controller exposes a write-end that the CLI feeds. By default we mirror the user's real stdin into the pipe (teeing so we can persist what was provided) and close it on EOF. Later scripted input can use the same interface.

4. **runtime_tracing integration**  
   `RuntimeTracer` keeps sole ownership of the `NonStreamingTraceWriter`. We wrap it in `Arc<Mutex<...>>` so the FD reader threads can call `TraceWriter::add_event(TraceLowLevelEvent::Event(...))`. Each chunk becomes an `EventLogKind::Write` (stdout), `WriteOther` (stderr), or `Read` (stdin) record whose `metadata` is a JSON document (stream, thread ids when known, activation state, step reference, timestamps) and whose `content` carries the base64-encoded bytes. IO data therefore lives in the same `trace.bin`/`trace.json` file as step and call events.

5. **Line attribution**  
   `RuntimeTracer` already handles `LINE` and `PY_*` monitoring callbacks. We extend it to track the most recent `Step` per Python thread and expose a `Snapshot` API returning `(path_id, line, call_key)`. When an IO chunk arrives we fetch the latest snapshot for the emitting Python thread (or fall back to the global latest step if the writer thread is unknown) and include it in the metadata so replay tooling can align output with execution.

6. **Recorder logging isolation**  
   Logger initialisation continues to route Rust `log` output to the preserved original stderr handle before redirection. Capture threads never use `println!`; they rely on the logger to avoid contaminating user streams.

7. **Buffering & flushing**  
   After installing the pipes we request line buffering on `sys.stdout`/`sys.stderr` when possible. On `stop()` we drain pending chunks, restore the original descriptors, and finish the runtime_tracing writer (`finish_writing_trace_*`).

---

## Alternatives Considered

- Monkey-patching `sys.stdout`/`sys.stderr`: misses native writes and conflicts with user overrides.
- Spawning a subprocess wrapper around the script: breaks the in-process monitoring story and changes CLI semantics.
- Emitting IO to a bespoke JSON artifact: unnecessary because runtime_tracing already models IO events.
- Deferring IO capture to a UI component: prevents parity with existing CodeTracer replay capabilities.

---

## Consequences

- **Pros**
  - IO, monitoring events, and metadata share the runtime_tracing stream, so downstream tooling continues to work.
  - Users keep invoking the CLI exactly the same way.
  - FD duplication captures writes coming from both Python and native extensions; recorder logging stays isolated.

- **Cons / Risks**
  - `NonStreamingTraceWriter` must be guarded for cross-thread use; we need to validate performance and correctness when `add_event` is called from background threads.
  - Mapping IO chunks to the exact Python thread is best-effort because file descriptors do not expose thread identity.
  - Reader threads must stay ahead of producers to avoid filling pipe buffers; shutdown paths must handle long-running readers.

---

## Detailed Design

### A. Execution lifecycle

1. **CLI (Python)**
   - Parse recorder flags, resolve `script_path`, choose format/path, call `start(trace_dir, format, start_on_enter=script_path)`.

2. **`start_tracing` (Rust)**
   - Initialise logging and guard against nested sessions.
   - Ensure the trace directory exists and choose event/meta/path filenames.
   - Instantiate `RuntimeTracer` with the requested format and activation path.
   - Wrap the tracer in `Arc<Mutex<RuntimeTracer>>` exposing `emit_io_chunk(...)`.
   - Construct `OutputCapture`:
     - Duplicate original fds/handles and store them for restoration and logger use.
     - Create pipes (Unix: `pipe2`; Windows: `CreatePipe`).
     - Redirect 0/1/2 via `dup2`/`SetStdHandle`.
     - Spawn reader threads that block on the pipe, timestamp (`Instant::now()`), gather OS thread id when available, and push `IoChunk { stream, bytes, ts, os_thread }` into a channel while forwarding the same bytes to the saved descriptors to maintain passthrough console behaviour.

3. **Tracing activation**
   - Install `sys.monitoring` callbacks with `install_tracer(py, Box::new(runtime_tracer_clone))`.
   - Start a draining thread that consumes `IoChunk`, resolves Python thread ids via a shared `DashMap<OsThreadId, PyThreadId>` maintained by `RuntimeTracer` on `PY_START/PY_RESUME`, and calls `emit_io_chunk`.

4. **Python script execution**
   - Back in Python, `runpy.run_path(str(script_path), run_name="__main__")` runs the target script. IO already flows through the capture pipelines.

5. **Shutdown**
- On normal completion or exception:
     - Close writer handles to signal EOF.
     - Join reader/draining threads with a timeout guard.
     - Restore original descriptors/handles.
     - Call `flush()`/`finish()` on `RuntimeTracer` and release the monitoring tool id.

### B. Encoding IO chunks with runtime_tracing

- `emit_io_chunk` composes a metadata JSON document similar to:

```json
{
  "stream": "stdout",
  "encoding": "base64",
  "ts_ns": 1234567890123,
  "os_thread": 140355679779840,
  "py_thread": 123,
  "step": { "path_id": 5, "line": 42, "call_key": 12 },
  "activation": "active"
}
```

- The payload is `base64::encode(chunk.bytes)`.
- We invoke `TraceWriter::add_event(TraceLowLevelEvent::Event(RecordEvent { kind, metadata, content }))`, where `kind` is `EventLogKind::Write` for stdout, `WriteOther` for stderr, and `Read` for stdin.
- This keeps IO data inside `trace.bin`/`trace.json`, allowing replay tools to process it alongside steps and returns.

### C. Mapping algorithm

- Maintain `DashMap<PyThreadId, StepSnapshot>` updated on every LINE event. A snapshot stores `(path_id, line, call_key, perf_counter_ns)`.
- When an IO chunk arrives:
  - Map OS thread id to Python thread id if possible; otherwise leave it null.
  - Retrieve the latest snapshot for that Python thread. If the chunk timestamp predates the snapshot by more than a configurable threshold, fall back to the global latest snapshot.
  - Embed the snapshot in the metadata JSON.
- For multi-line outputs the drainers keep chunk boundaries (newline-aware if the writer flushes per line) so replay can group contiguous chunks with identical snapshots.

---

## Implementation Notes (Unix/Windows)

- **Unix**: rely on the `nix` crate for `pipe2`, `dup`, `dup2`, and `fcntl` (set CLOEXEC). Reader threads use blocking `read` and propagate EOF by pushing a terminal message.
- **Windows**: use `CreatePipe`, `SetStdHandle`, `DuplicateHandle`, and `ReadFile` in overlapped mode if needed. Convert UTF-16 console output to UTF-8 before encoding. Ensure we re-register the original handles via `SetStdHandle` on teardown. Forwarding threads write mirrored bytes via the saved handles (`WriteFile`) so console output remains visible.
- PTY support (for interactive shells) can be layered later with `openpty`/ConPTY; initial scope sticks to pipes.

---

## Telemetry & Logging

- Keep `env_logger` initialisation on module import. Default to debug for the recorder crate so developers can inspect capture lifecycle logs.
- Emit debug logs (`install`, `chunk`, `drain_exit`, `restore`) to the preserved stderr handle. Emit failures as `EventLogKind::Error` events with diagnostic metadata when feasible.

---

## Key Files

- `codetracer-python-recorder/src/lib.rs`: orchestrates tracer startup/shutdown, trace directory provisioning, and will host creation of the `OutputCapture` component.
- `codetracer-python-recorder/src/runtime_tracer.rs`: owns the `NonStreamingTraceWriter`, handles `sys.monitoring` callbacks, and will expose APIs for emitting IO events plus step snapshots.
- `codetracer-python-recorder/src/tracer.rs`: manages registration with `sys.monitoring`; may require adjustments to share thread metadata with the capture pipeline.
- `codetracer-python-recorder/codetracer_python_recorder/__main__.py`: CLI entrypoint that invokes `start()`/`stop()`; may need updates for new flags or environment toggles.
- `codetracer-python-recorder/codetracer_python_recorder/api.py`: Python façade over the Rust extension, coordinating session lifecycle and flushing semantics.
- `codetracer-python-recorder/src/output_capture.rs` (new): encapsulates platform-specific descriptor duplication, pipe management, mirroring, and reader threads.
- `codetracer-python-recorder/tests/` (integration tests): will gain coverage asserting IO events appear in traces and that console passthrough remains functional.

---

## Testing Plan

1. **Unit / small integration**
   - Scripts emitting stdout/stderr; assert generated traces contain `EventLogKind::Write`/`WriteOther` records with correct base64 payloads.
   - Validate metadata JSON includes expected `path_id` and `line` for simple cases.

2. **Concurrency**
   - Multi-threaded printing to ensure no deadlocks and that chunks remain ordered.
   - `asyncio` tasks writing concurrently; confirm snapshots continue to resolve.

3. **Input**
   - `input()` and `sys.stdin.read()` scenarios; ensure `EventLogKind::Read` captures bytes and EOF.
   - Passthrough from the real stdin preserves exact bytes.

4. **Large output & stress**
   - Emit payloads larger than the pipe buffer to validate continuous draining.
   - Rapid start/stop cycles ensure descriptors restore cleanly.

5. **Windows**
   - Mirror the above coverage on Windows CI runners, focusing on handle restoration and CRLF handling.

---

## Rollout

- Gate the feature behind an environment flag (`CODETRACER_CAPTURE_IO=1`) for early adopters, removing it after validation.
- Update CLI help text to mention stdout/stderr/stdin capture.
- Add regression tests driven by `just test` that assert on IO events in traces generated from the examples.

---

## Open Questions / Future Work

- Improve thread attribution by integrating with Python `threading` hooks if OS-level mapping proves insufficient.
- Allow configurable passthrough (e.g., disable mirroring when running headless or redirect to a file) once the default teeing behaviour is in place.
- Investigate PTY support for interactive applications that expect terminal semantics.
- Consider compressing large payloads before base64 encoding to reduce trace sizes.

---

## Acceptance Criteria

- Running `python -m codetracer_python_recorder script.py` produces runtime_tracing files containing IO events alongside existing step/call records.
- stdout, stderr, and stdin bytes are captured losslessly and attributed to the most relevant step snapshot.
- Original descriptors are restored even on exceptions or early exits.
- Reader threads terminate cleanly without leaks on Unix and Windows.
