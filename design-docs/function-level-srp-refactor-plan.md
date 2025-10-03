# Function-Level SRP Plan (Quick Hits)

## Goal
Shrink the big tracer entry points so each one just coordinates helpers instead of doing everything inline.

## Main trouble spots
| Function | Issue |
| --- | --- |
| `session::start_tracing` | Mixes logging, state guards, filesystem setup, format parsing, argv capture, and tracer wiring. |
| Python `session.start` | Validates paths, normalises formats, toggles activation, and calls into Rust all in one go. |
| `RuntimeTracer::on_py_start` / `on_line` / `on_py_return` | Each does activation checks, frame poking, value capture, and logging in giant blocks. |

## Fix-it plan
1. **Create helpers first**
   - `session::bootstrap` for filesystem + format work.
   - `runtime::frame_inspector` to locate frames and pull locals/globals safely.
   - `runtime::value_capture` for arguments, scopes, and returns.
   - Python helpers for path + format validation.
2. **Trim the orchestration functions**
   - After helpers exist, each hotspot should read like: guard → call helper → send event.
3. **Test as we go**
   - Unit-test helpers with focused cases (bad paths, weird locals, async frames, etc.).
   - Keep running `just test` plus trace fixture diffs to ensure behaviour doesn’t change.
4. **Finish with cleanup**
   - Drop leftover TODOs, document the new helper modules, and mark ADR 0002 as accepted once the stages land.

## Done when
- No giant 60+ line functions remain in the tracer path.
- Unsafe frame handling lives inside audited helpers.
- Tests cover success, failure, and edge scenarios without needing full tracing sessions.
