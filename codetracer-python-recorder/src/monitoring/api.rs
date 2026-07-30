//! Monitoring API abstractions.

use std::any::Any;

use crate::code_object::CodeObjectWrapper;
use codetracer_trace_writer_nim::SpanRecord;
use pyo3::prelude::*;
use pyo3::types::PyAny;

use super::{CallbackOutcome, CallbackResult, EventSet, MonitoringEvents, NO_EVENTS};

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
}
