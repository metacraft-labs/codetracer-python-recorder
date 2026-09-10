//! Monitoring API abstractions.

use std::any::Any;

use crate::code_object::CodeObjectWrapper;
use codetracer_trace_writer_nim::SpanRecord;
use pyo3::prelude::*;
use pyo3::types::PyAny;

use super::{CallbackOutcome, CallbackResult, EventSet, MonitoringEvents, NO_EVENTS};

/// One correlation-marker declaration, on its way to the shared writer library.
///
/// A struct rather than eight positional parameters because this shape travels
/// through four layers (`#[pyfunction]` → `install` → [`Tracer`] → writer) and
/// a positional list that long silently tolerates two same-typed fields being
/// swapped.  Every field borrows: the strings were rendered by the host at the
/// PyO3 boundary, before any lock was taken, and are forwarded to the C ABI as
/// pointer + length without a copy or a NUL terminator.
///
/// Spec: `codetracer-specs/Testing/CTFS-Correlation-Marker-Contract.md` §11a.5.
///
/// There is deliberately NO step or geid field: a marker attaches to the
/// enclosing step and mints none of its own (§11a.6).
#[derive(Debug, Clone, Copy)]
pub struct CorrelationMarker<'a> {
    /// `Some` when the caller hoisted [`Tracer::ensure_marker_id`] out of its
    /// hot path — the primary path.  `None` selects the string-label wrapper,
    /// which interns inside the shared library so the ~20 recorders do not each
    /// grow a label cache that drifts (§11a.4).
    pub marker_id: Option<u64>,
    /// The pairing domain.  Two markers sharing it with opposing directions
    /// form a pair; it is also the TEXT the debugger displays, which is why it
    /// is passed alongside `marker_id` rather than instead of it.
    pub boundary: &'a str,
    /// `"send"` or `"recv"`.  The shared library normalises anything else to
    /// `"send"`: a marker with no side is unpairable, which is worse than one
    /// that picked a side.
    pub direction: &'a str,
    /// The already-rendered match value — what pairs the two sides.
    pub key_value: &'a str,
    /// An already-rendered payload shown alongside the key.  `""` for none.
    pub show_value: &'a str,
    /// Free-text description.  `""` for none.
    pub description: &'a str,
    /// The name `key_value` was read under.  `""` takes the library default.
    pub key_text: &'a str,
    /// The name `show_value` was read under.  `""` takes the library default.
    /// Load-bearing: an origin chain resumes its walk on this name in the
    /// sending recording.
    pub show_text: &'a str,
}

/// Trait implemented by tracing backends.
///
/// Each method corresponds to an event from `sys.monitoring`. Default
/// implementations allow implementers to only handle the events they care
/// about.
///
/// Every callback returns a `CallbackResult` so implementations can propagate
/// Python exceptions or request that CPython disables future events for a
/// location by yielding the `CallbackOutcome::DisableLocation` sentinel.
pub trait Tracer: Send + Any {
    /// Downcast support for implementations that need to be accessed
    /// behind a `Box<dyn Tracer>` (e.g., for flushing/finishing).
    fn as_any(&mut self) -> &mut dyn Any
    where
        Self: 'static,
        Self: Sized,
    {
        self
    }

    /// Return the set of events the tracer wants to receive.
    fn interest(&self, _events: &MonitoringEvents) -> EventSet {
        NO_EVENTS
    }

    /// Called on Python function calls.
    fn on_call(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _callable: &Bound<'_, PyAny>,
        _arg0: Option<&Bound<'_, PyAny>>,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called on line execution.
    fn on_line(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _lineno: u32,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called when an instruction is about to be executed (by offset).
    fn on_instruction(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called when a jump in the control flow graph is made.
    fn on_jump(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _destination_offset: i32,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called when a conditional branch is considered.
    fn on_branch(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _destination_offset: i32,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called at start of a Python function (frame on stack).
    ///
    /// Implementations should fail fast on irrecoverable conditions
    /// (e.g., inability to access the current frame/locals) by
    /// returning an error.
    fn on_py_start(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Notify the tracer that an unrecoverable error occurred and the runtime
    /// is transitioning into a detach/disable flow.
    fn notify_failure(&mut self, _py: Python<'_>) -> PyResult<()> {
        Ok(())
    }

    /// Provide the process exit status ahead of tracer teardown.
    fn set_exit_status(&mut self, _py: Python<'_>, _exit_code: Option<i32>) -> PyResult<()> {
        Ok(())
    }

    /// Called on resumption of a generator/coroutine (not via throw()).
    fn on_py_resume(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called immediately before a Python function returns.
    fn on_py_return(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _retval: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called immediately before a Python function yields.
    fn on_py_yield(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _retval: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called when a Python function is resumed by throw().
    fn on_py_throw(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _exception: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called when exiting a Python function during exception unwinding.
    fn on_py_unwind(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _exception: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called when an exception is raised (excluding STOP_ITERATION).
    fn on_raise(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _exception: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called when an exception is re-raised.
    fn on_reraise(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _exception: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called when an exception is handled.
    fn on_exception_handled(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _exception: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called on return from any non-Python callable.
    fn on_c_return(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _callable: &Bound<'_, PyAny>,
        _arg0: Option<&Bound<'_, PyAny>>,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Called when an exception is raised from any non-Python callable.
    fn on_c_raise(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _callable: &Bound<'_, PyAny>,
        _arg0: Option<&Bound<'_, PyAny>>,
    ) -> CallbackResult {
        Ok(CallbackOutcome::Continue)
    }

    /// Flush any buffered state to storage. Default is a no-op.
    fn flush(&mut self, _py: Python<'_>) -> PyResult<()> {
        Ok(())
    }

    /// Finish and close any underlying writers. Default is a no-op.
    fn finish(&mut self, _py: Python<'_>) -> PyResult<()> {
        Ok(())
    }

    /// RS-M5: append one span — a bounded, labeled interval of execution such
    /// as an HTTP request — to the trace container's span stream.
    ///
    /// This is the path the WSGI / ASGI middleware take instead of writing a
    /// `codetracer_spans.jsonl` sidecar: the span names a *(process, thread,
    /// step range)* coordinate INSIDE the container being recorded, so the
    /// Request Panel can seek from a row to the handler's first step.
    ///
    /// The default implementation returns an error rather than silently
    /// dropping the span, so a middleware is never told a request was recorded
    /// by a tracer that cannot record one.
    fn register_span(&mut self, _span: &SpanRecord) -> Result<(), String> {
        Err("the installed tracer does not support spans".to_string())
    }

    /// RS-M5: the step index the next recorded event will occupy — the
    /// `start_step` a span opened right now must carry.  See
    /// `NimTraceWriter::next_step_index` for why this must come from the writer
    /// and not from a recorder-side count of step registrations.
    ///
    /// `None` means "this tracer has no step timeline", which callers must
    /// distinguish from step 0.
    fn next_step_index(&self) -> Option<u64> {
        None
    }

    /// Intern a correlation-marker boundary label and return its id.
    ///
    /// The primary marker operation, mirroring path interning: a caller hoists
    /// it out of its hot path so [`Tracer::mark_correlation`] does no string
    /// lookup per crossing.
    ///
    /// The default ERRORS rather than returning a placeholder id.  Every other
    /// marker call keys on this id, so a backend that cannot intern cannot
    /// write a findable marker — and handing back an id that indexes nothing
    /// would produce markers that are *invisible* rather than broken, which is
    /// the exact failure this design exists to remove.
    fn ensure_marker_id(&mut self, _label: &str) -> Result<u64, String> {
        Err("the installed tracer does not support correlation markers".to_string())
    }

    /// Record one boundary crossing.  Mints no step — the marker attaches to
    /// the enclosing one (contract §11a.6).
    ///
    /// Errors by default, for the reason above: a caller told a boundary was
    /// recorded must not have been told so by a tracer that cannot record one.
    fn mark_correlation(&mut self, _marker: &CorrelationMarker<'_>) -> Result<(), String> {
        Err("the installed tracer does not support correlation markers".to_string())
    }

    /// Declare that this recording covers a distributed-trace span, from the
    /// hex ids an OTel API hands out.  Errors by default, as above.
    fn mark_span_coverage(
        &mut self,
        _trace_id_hex: &str,
        _span_id_hex: &str,
        _wall_time_unix_ns: u64,
        _monotonic_time_ns: u64,
    ) -> Result<(), String> {
        Err("the installed tracer does not support correlation markers".to_string())
    }
}
