# ADR 0012: Balanced sys.monitoring Call Stack Events

- **Status:** Proposed
- **Date:** 2025-10-26
- **Deciders:** codetracer recorder maintainers
- **Consulted:** Runtime tracing stakeholders, Replay consumers
- **Informed:** Support engineering, DX tooling crew

## Context
- The Rust-backed recorder currently subscribes to `PY_START`, `PY_RETURN`, and `LINE` events from `sys.monitoring`.
- `RuntimeTracer` only emits two structural trace records—`TraceWriter::register_call` and `TraceWriter::register_return`—because the trace file format has no explicit notion of yields, resumptions, or exception unwinding.
- Generators, coroutines, and exception paths trigger additional `sys.monitoring` events (`PY_YIELD`, `PY_RESUME`, `PY_THROW`, `PY_UNWIND`). When we ignore them the call stack in the trace becomes unbalanced, causing downstream tooling to miscompute nesting depth, duration, and attribution.
- CPython already exposes these events with complete callback metadata. We simply never hook them, so resumptions and unwinds silently skip our writer.

## Problem
- Trace consumers require balanced call/return pairs to reconstruct execution trees and propagate per-activation metadata (filters, IO capture, telemetry).
- When a generator yields, we never emit a `register_return`, so the activation remains "open" forever even if the generator is never resumed.
- When the interpreter unwinds a frame because of an exception, we neither emit a `register_return` nor mark the activation inactive, so the lifecycle bookkeeping leaks and `TraceWriter::finish_writing_trace_events` ends with dangling activations.
- Conversely, when a generator/coroutine resumes—either normally (`PY_RESUME`) or via `throw()` (`PY_THROW`)—we fail to emit the "call" edge that would push it back on the logical stack.
- Without these edges, the runtime cannot guarantee `TraceWriter` invariants or present accurate trace metadata. Adding synthetic bookkeeping in consumers is not possible because the events are already lost.

## Decision
1. **Treat additional monitoring events as structural aliases.**  
   - Map `PY_YIELD` and `PY_UNWIND` callbacks to the same flow as `on_py_return`, ultimately calling `TraceWriter::register_return`.  
   - Map `PY_RESUME` callbacks to the same flow as `on_py_start`, emitting a call edge with an empty argument vector because CPython does not provide the `send()` value (`https://docs.python.org/3/library/sys.monitoring.html#monitoring-event-PY_RESUME`).  
   - Map `PY_THROW` callbacks to the call flow but propagate the exception object as the payload recorded for the resumed activation so downstream tools can correlate the injected error; encode it as a single argument named `exception` using the existing value encoder (`https://docs.python.org/3/library/sys.monitoring.html#monitoring-event-PY_THROW`).
2. **Subscribe to the four events in `RuntimeTracer::interest`.** The tracer will request `{PY_START, PY_RETURN, PY_YIELD, PY_UNWIND, PY_RESUME, PY_THROW}` plus `LINE` to preserve current behaviour.
3. **Unify lifecycle hooks.** Extend the activation manager so that yield/unwind events deactivate the frame and resumption events reactivate or spawn a continuation while preserving filter decisions, telemetry handles, and IO capture state.
4. **Preserve file-format semantics.** We will not add new record types; instead we ensure every control-flow boundary ultimately produces the same call/return records the file already understands.
5. **Defensive guards.** Log-and-disable behaviour stays unchanged: any callback failure still honours policy (`OnRecorderError`). The new events use the same `should_trace_code` and activation gates so filters can skip generators consistently.

## Consequences
- **Benefits:**  
  - Balanced call stacks for generators, coroutines, and exception unwinds without touching the trace schema.  
  - Replay and analysis tools stop seeing "dangling activation" warnings, improving trust in exported traces.  
  - The recorder can later add richer semantics (e.g., value capture on resume) because the structural foundation is sound.
- **Costs:**  
  - Slightly higher callback volume, especially in generator-heavy workloads (two extra events per yield/resume pair).  
  - Additional complexity inside `RuntimeTracer` to differentiate return-like vs call-like flows while sharing writer helpers.
- **Risks:**  
  - Incorrect mapping could double-emit calls or returns, corrupting the trace. We mitigate this with targeted tests covering yields, exceptions, and `throw()`-driven resumes.  
  - Performance regressions if the new paths capture values unnecessarily; we will keep value capture opt-in via filter policies.

## Alternatives
- **Introduce new trace record kinds for each event.** Rejected because consumers, storage, and analytics would all need format upgrades, and the existing stack-only writer already conveys the necessary structure.
- **Approximate via Python-side bookkeeping.** Rejected: the Python helper cannot observe generator unwinds once the Rust tracer suppresses the events.
- **Ignore stack balancing and patch consumers.** Rejected because it hides the source of truth and still leaves us without activation lifecycle signals during recording (IO capture, telemetry).

## Key Examples

### 1. Ordinary Function Call
```python
def add(a, b):
    return a + b

result = add(4, 5)
```
- `PY_START` fires when `add` begins. We capture the two arguments via `capture_call_arguments` and call `TraceWriter::register_call(function_id=add, args=[("a", 4), ("b", 5)])`.
- `PY_RETURN` fires just before the return. We record the value `9` through `record_return_value`, which invokes `TraceWriter::register_return(9)`.
- The trace shows a single balanced call/return pair; no other structural events are emitted.

### 2. Generator Yield + Resume
```python
def ticker():
    yield "ready"
    yield "again"

g = ticker()
first = next(g)
second = next(g)
```
- First `next(g)`:
  - `PY_START` → `register_call(ticker, args=[])`.
  - `PY_YIELD` → `register_return("ready")`. The activation is now suspended but the trace stack is balanced.
- Second `next(g)`:
  - `PY_RESUME` → `register_call(ticker, args=[])` (empty vector because CPython does not expose the send value).
  - `PY_YIELD` → `register_return("again")`.
- When the generator exhausts, CPython emits `PY_RETURN`, so we `register_return(None)` (or whatever value was returned). Every suspension/resumption pair corresponds to alternating `register_return`/`register_call`, keeping the call stack consistent.

### 3. Generator Throw
```python
def worker():
    try:
        yield "ready"
    except RuntimeError as err:
        return f"caught {err}"

g = worker()
next(g)
g.throw(RuntimeError("boom"))
```
- Initial `next(g)` behaves like Example 2.
- `g.throw(...)` triggers:
  - `PY_THROW` with the exception object. We emit `register_call(worker, args=[("exception", RuntimeError("boom"))])`, encoding the exception with the existing value encoder so it appears in the trace payload.
  - If the generator handles the exception and returns, `PY_RETURN` follows and we write `register_return("caught boom")`. If it re-raises, `PY_UNWIND` fires instead and we encode the exception value in `register_return`.

### 4. Exception Unwind Without Yield
```python
def explode():
    raise ValueError("bad news")

def run():
    return explode()

run()
```
- `explode()` starts: `PY_START` → `register_call(explode, args=[])`.
- The function raises before returning, so CPython skips `PY_RETURN` and emits `PY_UNWIND` with the `ValueError`.
- We treat `PY_UNWIND` like `PY_RETURN`: flush pending IO, encode the exception via `record_return_value`, and call `register_return(ValueError("bad news"))`. The activation controller marks the frame inactive, preventing dangling stack entries when tracing finishes.

### 5. Coroutine Await / Resume
```python
import asyncio

async def worker():
    await asyncio.sleep(0)
    return "done"

asyncio.run(worker())
```
- Entry: `PY_START` → `register_call(worker, args=[])`.
- When the coroutine awaits `sleep(0)`, CPython emits `PY_YIELD` with no explicit value (await results are delivered later). We encode the pending await result (typically `None`) via `register_return`.
- When the event loop resumes `worker`, `PY_RESUME` fires and we record another `register_call(worker, args=[])`. No payload is available because the resume value is implicit in the await machinery.
- Final completion triggers `PY_RETURN` so we write `register_return("done")`.
- The trace therefore shows multiple call/return pairs for the same coroutine activation, mirroring each suspend/resume cycle.

## Rollout
1. Update the design docs with this ADR and the implementation plan.  
2. Implement the runtime changes behind standard CI, landing tests that prove stack balance for yields, unwinds, and resumes.  
3. Notify downstream consumers that generator traces now appear balanced without requiring schema or API changes.  
4. Monitor regression dashboards for callback volume and latency after enabling the new events by default.
