In Python monitoring sometimes the co_filename of a code object
doesn't point to a real file, but something else. Those filenames look
like `<...>`.

Lines from those files cannot be traced. For this reason, we should
skip them for all monitoring events.

sys.monitoring provides the capability to turn off monitoring for
specific lines by having the callback return a special value
`sys.monitoring.DISABLE`. We want to use this functionality to disable
monitoring of those lines and improve performance.

The following changes need to be made:

1. Extend the `Tracer` trait so every callback can signal back a
   `sys.monitoring` action (continue or disable). Update all existing
   implementations and tests to use the new return type.
2. Add reusable logic that decides whether a given code object refers
   to a real on-disk file and cache the decision per `co_filename` /
   code id.
3. Invoke the new filtering logic from every `RuntimeTracer` callback
   before any expensive work. When a code object should be ignored,
   skip our bookkeeping and return the disable sentinel to CPython so
   further events from that location stop firing.

Note: We cannot import `sys.monitoring` inside the hot callbacks,
because in some embedded runtimes importing during tracing is either
prohibited or will deadlock. We must therefore cache the
`sys.monitoring.DISABLE` sentinel ahead of time while we are still in a
safe context (e.g., during tracer installation).

We need to make sure that our test suite has comprehensive tests that
prove the new filtering/disable behaviour and cover regressions on the
public tracer API.

# Technical design solutions

## Tracer callback return values

- Introduce a new enum `CallbackOutcome` in `src/tracer.rs` with two
  variants: `Continue` (default) and `DisableLocation`.
- Define a `type CallbackResult = PyResult<CallbackOutcome>` so every
  trait method can surface Python errors and signal whether the
  location must be disabled. `Continue` replaces the current implicit
  unit return.
- Update the `Tracer` trait so all callbacks return `CallbackResult`.
  Default implementations continue to return `Ok(CallbackOutcome::Continue)`
  so existing tracers only need minimal changes.
- The PyO3 callback shims (`callback_line`, `callback_py_start`, etc.)
  will translate `CallbackOutcome::DisableLocation` into the cached
  Python sentinel and otherwise return `None`. This keeps the Python
  side compliant with `sys.monitoring` semantics
  (see https://docs.python.org/3/library/sys.monitoring.html#sys.monitoring.DISABLE).

## Accessing `sys.monitoring.DISABLE`

- During `install_tracer`, after we obtain `monitoring_events`, load
  `sys.monitoring.DISABLE` once and store it in the global tracer state
  (`Global` struct) as a `Py<PyAny>`. Because `Py<PyAny>` is `Send`
  + `Sync`, it can be safely cached behind the global mutex and reused
  inside callbacks without re-importing modules.
- Provide a helper on `Global` (e.g., `fn disable_sentinel<'py>(&self,
  py: Python<'py>) -> Bound<'py, PyAny>`) that returns the bound object
  when we need to hand the sentinel back to Python.
- Make sure `uninstall_tracer` drops the sentinel alongside other
  state so a new install can reload it cleanly.

## `RuntimeTracer` filename filtering

- Add a dedicated method `fn should_trace_code(&mut self,
  py: Python<'_>, code: &CodeObjectWrapper) -> ShouldTrace` returning a
  new internal enum `{ Trace, SkipAndDisable }`.
  - A file is considered “real” when `co_filename` does not match the
    `<...>` pattern. For now we treat any filename that begins with `<`
    and ends with `>` (after trimming whitespace) as synthetic. This
    covers `<frozen importlib._bootstrap>`, `<stdin>`, `<string>`, etc.
  - Cache negative decisions in a `HashSet<usize>` keyed by the code
    object id so subsequent events avoid repeating the string checks.
    The set is cleared on `flush()`/`finish()` if we reset state.
- Each public callback (`on_py_start`, `on_line`, `on_py_return`) will
  call `should_trace_code` first. When the decision is `SkipAndDisable`
  we:
  - Return `CallbackOutcome::DisableLocation` immediately so CPython
    stops sending events for that location.
  - Avoid calling any of the expensive frame/value capture paths.
- When the decision allows tracing, we continue with the existing
  behaviour. The activation-path logic runs before the filtering so a
  deactivated tracer still ignores events regardless of filename.

## Backwards compatibility and ergonomics

- `RuntimeTracer` becomes the only tracer that returns
  `DisableLocation`; other tracers keep returning `Continue`.
- Update the test helper tracers under `tests/` to use the new return
  type but still assert on event counts; their filenames will remain
  real so behaviour does not change.
- Document the change in the crate-level docs (`src/lib.rs`) to warn
  downstream implementors that callbacks now return `CallbackResult`.

# Test suite

- Rust unit test for the pure filename predicate (e.g.,
  `<string>`, `<frozen importlib._bootstrap>`, `script.py`) to prevent
  regressions in the heuristic.
- Runtime tracer integration test that registers a `RuntimeTracer`,
  executes code with a `<string>` filename, and asserts that:
  - No events are written to the trace writer.
  - The corresponding callbacks return the disable sentinel (inspect
    via a lightweight shim or mock writer).
- Complementary test that runs a real file (use `tempfile` to emit a
  small script) and ensures events are still recorded.
- Regression tests for the updated trait: adjust `tests/print_tracer.rs`
  counting tracer to assert it still receives events and that the
  return value defaults to `Continue`.
- Add a smoke test checking we do not attempt to import
  `sys.monitoring` inside callbacks by patching the module import hook
  during a run.

# Implementation Plan

1. Introduce `CallbackOutcome`/`CallbackResult` in `src/tracer.rs` and
   update every trait method signature plus the PyO3 callback shims.
   Store the `sys.monitoring.DISABLE` sentinel in the `Global` state.
2. Propagate signature updates through existing tracers and tests,
   ensuring they all return `CallbackOutcome::Continue`.
3. Extend `RuntimeTracer` with the filename filtering method, cached
   skip set, and early-return logic that emits `DisableLocation` when
   appropriate.
4. Update the runtime tracer callbacks (`on_py_start`, `on_line`,
   `on_py_return`, and any other events we wire up later) to invoke the
   filtering method first.
5. Expand the test suite with the new unit/integration coverage and
   adjust existing tests to the trait changes.
6. Perform a final pass to document the new behaviour in public docs
   and ensure formatting/lints pass.
