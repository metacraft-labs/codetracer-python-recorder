# Value Capture Plan (Bite Size)

## Goal
For every line event, record every variable the frame can see (locals, nonlocals, globals) without crashing CPython.

## How to grab data
1. Get the active frame with `PyEval_GetFrame()` (or `PyThreadState_GetFrame`).
2. Call `PyFrame_FastToLocalsWithError` before reading mappings.
3. Pull locals (includes closure vars in 3.12+) via `PyFrame_GetLocals`.
4. Pull globals via `PyFrame_GetGlobals`; skip if it’s the same dict as locals.
5. Encode each entry through our existing `encode_value` helper and write to the trace.
6. Keep builtins out of the snapshot.

## Edge rules
- Treat filenames like `<stdin>` elsewhere; value capture only runs when the tracer says the code is traceable.
- Frames for comprehensions, generators, and class bodies work the same—each has its own locals dict.
- Watch for locals==globals (module/class bodies); avoid double-recording.
- If encoding fails, surface a `RecorderError` and mark the event partial.

## Safety
- Hide all raw pointer work inside a dedicated `frame_inspector` module with RAII guards.
- Ensure helpers run under the GIL and clean up borrowed references.

## Test checklist
- Simple function: parameters and locals appear as they are assigned.
- Nested closure: inner frame sees outer variables via locals proxy.
- Globals + `global` statement: updates reflect in module scope.
- Class body with metaclass: captures class attributes during definition.
- Comprehensions, lambdas, generator expressions: loop vars captured during execution, not leaked after.
- Generator + async def: values persist across yield/await boundaries.
- Exception handler: `except as err` exposes the alias.
- Context manager `with` target and walrus assignments show up once bound.
- Ensure cycle detection prevents infinite recursion when encoding self-referential structures.
