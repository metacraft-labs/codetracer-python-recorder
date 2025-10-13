# Skip Fake Filenames Plan

## Problem
Some code objects report filenames like `<stdin>` or `<string>`. Those are synthetic, so tracing them wastes work and breaks source lookups.

## Fix
1. **Let callbacks opt out**
   - `Tracer` methods return a `CallbackOutcome` (`Continue` or `DisableLocation`).
   - PyO3 shims translate `DisableLocation` into the cached `sys.monitoring.DISABLE` sentinel.
2. **Cache the sentinel**
   - Load `sys.monitoring.DISABLE` during installation and keep it in global state as a `Py<PyAny>`.
3. **Filter in `RuntimeTracer`**
   - Add `should_trace_code` that treats filenames wrapped in `<...>` as fake.
   - Cache the skip decision per code object id so we bail out fast next time.
   - If fake, return `DisableLocation` immediately and skip all heavy lifting.

## Tests
- Unit test the filename predicate (`<string>` vs `script.py`).
- Runtime test confirming `<string>` code triggers the disable path and real files still trace.
- Update helper tracers/tests to use the new return type.

## Done when
- Synthetic filenames stop generating events.
- Real files still trace normally.
- No callback imports `sys.monitoring` on the hot path.
