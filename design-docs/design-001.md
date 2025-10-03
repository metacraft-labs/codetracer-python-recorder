# Python Monitoring Tracer (Cheat Sheet)

## Goal
Turn `sys.monitoring` events into the `runtime_tracing` stream so we can record Python programs without patching CPython.

## Moving parts
- **Tool startup**
  - Grab a tool id with `sys.monitoring.use_tool_id("codetracer")`.
  - Load the event constants and register one callback per event we care about.
  - Enable those events in one `set_events` call.
- **Dispatcher**
  - Implement the `Tracer` trait so each callback receives only the events it opts into (bit mask filter).
  - Each callback also receives the `CodeObjectWrapper` described in the wrapper doc.
- **Trace writer**
  - Open a JSON or binary writer when tracing starts.
  - Append metadata and source files up front.
  - Flush and close cleanly on shutdown.
- **Thread + activation tracking**
  - Keep a per-thread stack of activation ids that mirrors CALL → RETURN / YIELD → RESUME.
  - Record the first event per thread as “thread started” and clean up on the last event.
  - Store the code object id and current offset/line on the activation record.

## Event map (high level)
| Monitoring event | What we log |
| --- | --- |
| `CALL`, `PY_START` | Push a new activation, record the function entry. |
| `LINE`, `INSTRUCTION`, `BRANCH`, `JUMP` | Write step/control-flow events with filename + line/offset. |
| `PY_RETURN`, `PY_YIELD`, `STOP_ITERATION` | Pop/flag the activation and note the value if we can encode it. |
| `EXCEPTION_HANDLED`, `PY_THROW`, `PY_UNWIND`, `RERAISE`, `C_RAISE`, `C_RETURN` | Emit error or C-API bridge events so time-travel tools can follow the story. |
| `PY_RESUME` | Mark that the paused activation is running again. |

## Data helpers
- Use the global `CodeObjectRegistry` to avoid repeated getattr calls.
- When we need line tables, call `code.co_lines()` once and cache the entries inside the wrapper.
- Track thread state with a `DashMap<ThreadId, ThreadState>`; use thread-local fallback if necessary.

## Safety rails
- Do not touch `PyCodeObject` internals—only public attributes via PyO3.
- Keep callbacks tiny: grab the wrapper, record the event, hand off to the writer.
- If a callback fails, surface a structured `RecorderError` and disable tracing for that thread so we fail safe.

## Done when
- A simple Python script traced through this pipeline produces a valid `trace.json` / `trace.bin` compatible with the rest of Codetracer.
- Activations balance correctly across nested calls, yields, and exceptions.
- Profiling shows no per-event Python attribute churn (thanks to the wrapper cache).
