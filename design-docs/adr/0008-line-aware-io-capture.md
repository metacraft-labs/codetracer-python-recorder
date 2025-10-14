# ADR 0008: Line-Aware IO Capture Through Stream Proxies

- **Status:** Proposed
- **Date:** 2025-01-14
- **Supersedes:** ADR 0007
- **Deciders:** Runtime recorder maintainers
- **Consulted:** Python platform crew, Replay tooling crew
- **Informed:** DX crew, Release crew

## Context
- We must attribute every visible chunk of IO to the Python line that triggered it.
- Pipe-based capture lags behind the interpreter and breaks the ordering with our line events.
- The refactored recorder already tracks thread snapshots for line events and ships a policy system plus lifecycle hooks.
- Patching `sys.stdout` and friends is the only way to synchronise output with the active frame without changing how users launch their code.

## Problem
- We need an in-process IO capture layer that keeps pass-through behaviour, works across CPython versions we support, and does not swallow our own logs.
- We must cover writes coming from Python code and from C extensions that call the CPython stream APIs.
- The solution must restore the original streams even if tracing crashes or the user stops tracing inside a `finally` block.

## Decision
1. Introduce `runtime::io_lines` with one public type, `IoStreamProxies`. It owns the original `{stdout, stderr, stdin}` objects and exposes `install(py)` / `uninstall(py)` helpers.
2. Provide three PyO3 classes: `LineAwareStdout`, `LineAwareStderr`, and `LineAwareStdin`. They proxy every method we rely on (`write`, `writelines`, `flush`, `read`, `readline`, `readinto`, iteration).
3. Each proxy calls back into Rust. The callback grabs the per-thread `LineSnapshot` maintained by the monitoring layer. When the snapshot is missing we record `None` and mark the IO chunk as "detached".
4. The callback forwards the payload to the original stream right away so the console stays live. We record the chunk in an `IoEventSink` right after the forward call while we still hold the GIL.
5. `IoEventSink` batches small writes per thread to reduce chatter. A flush, newline, line switch, or time gap of 5 ms emits the batch. When the monitoring layer emits a Step event for the same thread it first flushes the pending IO batch, then writes the Step, then installs the new snapshot. This keeps the Step/IO ordering deterministic: `Step -> IO emitted by that Step -> next Step`.
6. Extend `RuntimeTracer` with an `io_capture` field. `start_tracing` installs the proxies after the recorder policy authorises IO capture. `stop_tracing` and panic handlers call `teardown_io_capture`, which drains pending batches and restores the original streams.
7. Guard our own logging with a `ScopedMuteIoCapture` RAII helper. It sets a thread-local flag so proxy callbacks short-circuit when the recorder writes to stderr.
8. Add an optional best-effort FD mirror for `stdout`/`stderr`. When enabled it duplicates the file descriptors and spawns a reader that only handles writes not seen by the proxies. We track bytes seen by proxies in a per-stream ledger that keeps a FIFO byte buffer plus a monotonic sequence ID. The mirror removes ledger bytes from every read chunk with a streaming diff: scan left to right, skip native bytes, and peel ledger entries whenever their bytes appear, even when native writes arrive first in the chunk. Whatever bytes remain become mirror-only output. The GIL keeps Python `write` calls serial, so proxy order matches the order the OS sees. When native code writes directly to the FD it appears in the diff as leftover bytes we record with a `FdMirror` source tag.
9. Expose a policy flag `policy.io_capture.line_proxies` defaulting to `true`. The FD mirror stays off by default and hides behind `policy.io_capture.fd_fallback`.

## Consequences
- **Pros:** We align IO chunks with the current Python frame, match C extensions that honour `sys.stdout`, and keep console behaviour untouched. The design lives inside the existing lifecycle code.
- **Cons:** The proxies add overhead to every write call and must stay in sync with CPython `TextIOBase`. We have to maintain batching logic and reentrancy guards.
- **Risks:** Misbehaving third-party code that replaces `sys.stdout` mid-run may bypass us. If the user hands a binary stream to `sys.stdout` we must fall back to passthrough mode. The FD mirror ledger can fall behind if proxies skip `record_proxy_bytes`. We detect the mismatch, reset the ledger, and keep the bytes via the mirror so capture stays lossless even when dedupe fails.

## Rollout
- Ship behind the new policy flags and an environment override `CODETRACER_CAPTURE_IO=proxies[,fd]`.
- Keep telemetry for dropped chunks and proxy failures. Emit a single warning when the proxy install fails and we fall back to plain pass-through.
- Promote this ADR to **Accepted** once the implementation plan ships on Linux and Windows and soak tests confirm line attribution accuracy.

## Alternatives
- Keep the old pipe-based capture: rejected because it can never align output timing with the interpreter.
- Subclass Python `io.TextIOWrapper` in pure Python: rejected because the Rust-backed recorder needs control over batching and logging guards inside the GIL.
- Patch `libc::write` through LD_PRELOAD: rejected as too invasive and brittle across platforms. It also cannot recover the active Python frame.
