# ADR: Non-invasive stdout/stderr/stdin capture and line-level mapping for the Python tracer (PyO3 + `sys.monitoring`)

**Status**: Accepted
**Date**: 2025-10-01
**Owners**: Tracing/Recorder team
**Scope**: Recorder runtime (Rust/PyO3) and Python instrumentation glue

---

## Context

We are building a Python tracing recorder in Rust (PyO3) that:

* runs a target Python script (3.12+),
* captures **stdout/stderr** without modifying user code,
* feeds/captures **stdin**,
* maps emitted output to the **source line** that produced it,
* records events for **post-mortem replay** (not live UI),
* works with **threads** and **asyncio**,
* allows **Rust-side logging** that does not contaminate captured stdout/stderr.

Current runner uses `runpy.run_path` from Python space; we now need the same behavior when launching from Rust to enable FD-level capture.

---

## Decision

1. **Output capture method**:
   Use **file-descriptor (FD) redirection** with **pipes** (or PTY when TTY semantics required):

   * Redirect **stdout(1)** and **stderr(2)** to pipes via `dup2` (Unix) / `SetStdHandle` (Windows).
   * Drain both ends concurrently in background threads; timestamp each chunk.

2. **Input capture/feeding**:
   Redirect **stdin(0)** to a pipe. The controller writes scripted input (or forwards from the original stdin) and **closes** the write end to signal EOF. Optionally tee input to a log for replay.

3. **Execution model** (same semantics as `runpy.run_path`):
   From Rust/PyO3, call **`runpy.run_path(path, run_name="__main__")`** after:

   * setting `sys.argv`,
   * setting `sys.path[0]` to the script directory,
   * `chdir` to the script directory.

4. **Tracing API**:
   Use Python 3.12+ **`sys.monitoring`**:

   * Allocate a dedicated **tool_id**.
   * Enable at minimum **LINE** events globally (`set_events`), optionally **CALL/C_RETURN** for finer disambiguation.
   * Register a **lightweight callback** that records `(ts, thread_ident, filename, func_name, line_no)`.

5. **Mapping outputs → lines**:
   Merge the two time-ordered streams:

   * For each output chunk, associate with the **most recent preceding** LINE event (per-thread if thread identifiable; otherwise global order).
   * Store `(line_ref, stream, bytes, ts_start, ts_end)` records for replay.

6. **Rust logging isolation**:
   Initialize a logger that writes to **the saved original stderr FD** (pre-redirection) or to a dedicated file/syslog. **Never** use `println!/eprintln!` after redirection.

7. **Buffering & latency**:
   For timeliness (optional), set `sys.stdout.reconfigure(line_buffering=True)` / same for stderr. Not strictly required for correctness.

---

## Alternatives Considered

* **Monkey-patch `sys.stdout`/`sys.stderr`**: rejected (misses C-level writes, invasive, behavior-changing).
* **Only `sys.settrace`/`sys.monitoring` without FD capture**: rejected (no output capture from C/native extensions).
* **Use C-API `PyRun_*` instead of `runpy`**: viable but more edge-cases; `runpy.run_path` already matches CLI semantics reliably.
* **Single combined PTY**: good for interactive TTY behavior, but merges stdout/err; choose per use-case.

---

## Consequences

* **Pros**:

  * Captures all output (Python & C extensions) non-invasively.
  * Accurate line mapping via `sys.monitoring`.
  * Works with threads/async; deterministic post-mortem merging via timestamps.
  * Debug logging remains out-of-band.

* **Cons / Risks**:

  * FD redirection is process-wide; be careful with concurrent embeddings.
  * Pipes can block if not drained—must have dedicated reader threads.
  * Interleaved writes from multiple threads may produce mixed chunks; per-thread mapping is best-effort.
  * Slight runtime overhead from LINE callbacks; CALL/C_RETURN add more.

---

## Detailed Design

### A. Runner lifecycle (Rust)

1. **Prepare capture**

   * `pipe()` for out/err/in.
   * `dup()` and save originals for 0/1/2.
   * `dup2` to redirect: `1→out_w`, `2→err_w`, `0→in_r`. Close extra ends.

2. **Start drainers**

   * Thread A: read `out_r` (blocking), timestamp, append to `stdout` buffer/queue.
   * Thread B: read `err_r`, same for `stderr`.

3. **(Optional) stdin strategy**

   * Scripted: write bytes to `in_w`, then close to signal EOF.
   * Passthrough: thread C copies from **saved** original stdin to `in_w` and also logs for replay.

4. **Initialize Python**

   * Acquire GIL.
   * Enable monitoring: `use_tool_id`, `register_callback(LINE)`, `set_events(LINE | optional CALL/C_RETURN)`.
   * Set `sys.argv`, adjust `sys.path[0]`, `chdir`.

5. **Run target**

   * `runpy.run_path(path, run_name="__main__")`.
   * On exception, format traceback and record as a structured event.

6. **Teardown**

   * `set_events(tool_id, 0)`, `register_callback(tool_id, LINE, None)`.
   * Close `in_w` if not already.
   * Drain remaining output; restore original FDs via `dup2(saved, fd)`; close all pipe FDs.

7. **Merge & persist**

   * Merge LINE events and output chunks by timestamp (per-thread if available).
   * Persist a session artifact:

     ```json
     {
       "env": {...},
       "events": [ { "ts":..., "thread":..., "file":..., "func":..., "line":... } ],
       "io": [
         { "ts0":..., "ts1":..., "stream":"stdout", "thread":..., "bytes":"base64..." },
         { "ts0":..., "ts1":..., "stream":"stderr", ... }
       ],
       "stdin": [ ... ]
     }
     ```

### B. Monitoring callback (Python or Rust)

* **Python shim** (simple): callback appends tuples to a Python list; Rust extracts at end.
* **Rust callback** (preferred for perf): expose `#[pyfunction]` that pushes to a lock-free ring buffer while holding the GIL briefly.

**Event fields**: `ts = perf_counter() (or Rust monotonic)`, `thread_ident = threading.get_ident()`, `code.co_filename`, `code.co_name`, `line_no`.

### C. Mapping algorithm

* Maintain `last_line_by_thread` updated on each LINE event.
* When an output chunk arrives:

  * Assign `thread = current “owner”` if known; else leave `null` and fall back to global last line.
  * Attribute to `last_line_by_thread[thread]` (or global last line).
* For fine disambiguation on the same source line:

  * Optionally enable **CALL/C_RETURN** and attribute chunks occurring between them to the active call frame (e.g., `print` invocations).

---

## Implementation Notes (Unix/Windows)

* **Unix**: use `nix` crate for `pipe/dup/dup2/read/write/close`.
* **Windows**: `CreatePipe`, `SetStdHandle`, `GetStdHandle` to save originals; use `ReadFile`/`WriteFile` in reader/feeder threads.
* **TTY needs**: use a **PTY** (`openpty`/`forkpty` or `winpty/ConPTY`) to provide terminal behavior (echo, line editing).

---

## Telemetry & Logging

* Initialize a custom logger writing to the **saved original stderr FD** or to a **file/syslog** before enabling redirection.
* Include a **session header** (versions, timestamps, script path, argv) in the persisted artifact.

---

## Testing Plan

1. **Unit**

   * Pure-Python prints (single/multi-line, with/without flush).
   * C-level prints (e.g., `ctypes` calling `puts`, NumPy warning to stderr).
   * Exceptions (traceback captured and not lost in pipes).

2. **Concurrency**

   * Multiple threads printing interleaved; verify mapping is stable and no deadlocks (drainers running).
   * `asyncio` tasks printing; verify sequence is coherent.

3. **Input**

   * `input()` / `sys.stdin.read()` with scripted stdin; ensure EOF ends read.
   * Passthrough mode; ensure tee log matches bytes fed.

4. **Behavioral**

   * Relative imports from script dir work (sys.path[0]/cwd).
   * Large outputs (≥ pipe buffer) do not deadlock; throughput OK.

5. **Windows**

   * Equivalent redirection and restore; CRLF handling; code page sanity.

---

## Rollout

* Implement behind a feature flag (`runner_fd_capture`).
* Ship a CLI subcommand to run a script with capture for manual validation.
* Gate by runtime check: Python ≥ 3.12.
* Add an integration test matrix (Linux/macOS/Windows).

---

## Open Questions / Future Work

* Add optional **INSTRUCTION** events for ultra-fine mapping when needed.
* Detect and label **subprocess** outputs (inherit our FDs? PTY? wrappers).
* Expose a **live tee** to developer console while still recording (mirror to saved original fds).
* Structured replay API (seek by time/line/thread; fold/expand calls).
* Consider **no-GIL** Python in future: ensure event buffers are thread-safe without relying on GIL serialization.

---

## Acceptance Criteria

* Captures stdout/stderr/stdin non-invasively; no contamination from recorder logs.
* Produces a stable mapping from output chunks to source lines for typical code, threads, and asyncio.
* Equivalent semantics to `runpy.run_path(..., run_name="__main__")`.
* Clean startup/teardown with restored FDs; no deadlocks/leaks on large I/O.
* Cross-platform (Linux/macOS; Windows parity planned or implemented per milestone).
