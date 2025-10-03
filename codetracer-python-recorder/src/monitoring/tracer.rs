//! Tracer trait and sys.monitoring callback plumbing.

use std::any::Any;
use std::sync::Mutex;

use crate::code_object::{CodeObjectRegistry, CodeObjectWrapper};
use crate::ffi;
use crate::policy::{self, OnRecorderError};
use pyo3::{
    prelude::*,
    types::{PyAny, PyCode, PyModule},
};
use recorder_errors::{usage, ErrorCode};
use log::{error, warn};

use super::{
    acquire_tool_id, free_tool_id, monitoring_events, register_callback, set_events,
    CallbackOutcome, CallbackResult, EventSet, MonitoringEvents, ToolId, NO_EVENTS,
};

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

    /// Called when an artificial StopIteration is raised.
    // Tzanko: I have been unable to write Python code that emits this event. This happens both in Python 3.12, 3.13
    // Here are some relevant discussions which might explain why, I haven't investigated the issue fully
    // https://github.com/python/cpython/issues/116090,
    // https://github.com/python/cpython/issues/118692
    // fn on_stop_iteration(
    //     &mut self,
    //     _py: Python<'_>,
    //     _code: &CodeObjectWrapper,
    //     _offset: i32,
    //     _exception: &Bound<'_, PyAny>,
    // ) {
    // }

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
}

struct Global {
    registry: CodeObjectRegistry,
    tracer: Box<dyn Tracer>,
    mask: EventSet,
    tool: ToolId,
    disable_sentinel: Py<PyAny>,
}

static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);

fn handle_callback_result(
    py: Python<'_>,
    guard: &mut Option<Global>,
    result: CallbackResult,
) -> PyResult<Py<PyAny>> {
    match result {
        Ok(CallbackOutcome::Continue) => Ok(py.None()),
        Ok(CallbackOutcome::DisableLocation) => Ok(
            guard
                .as_ref()
                .map(|global| global.disable_sentinel.clone_ref(py))
                .unwrap_or_else(|| py.None()),
        ),
        Err(err) => handle_callback_error(py, guard, err),
    }
}

fn handle_callback_error(
    py: Python<'_>,
    guard: &mut Option<Global>,
    err: PyErr,
) -> PyResult<Py<PyAny>> {
    let policy = policy::policy_snapshot();
    match policy.on_recorder_error {
        OnRecorderError::Abort => Err(err),
        OnRecorderError::Disable => {
            let message = err.to_string();
            error!(
                "recorder callback error; disabling tracer per policy: {}",
                message
            );
            if let Some(global) = guard.as_mut() {
                if let Err(notify_err) = global.tracer.notify_failure(py) {
                    warn!(
                        "failed to notify tracer about disable transition: {}",
                        notify_err
                    );
                }
            }
            uninstall_locked(py, guard)?;
            Ok(py.None())
        }
    }
}

fn uninstall_locked(py: Python<'_>, guard: &mut Option<Global>) -> PyResult<()> {
    if let Some(mut global) = guard.take() {
        let _ = global.tracer.finish(py);
        let events = monitoring_events(py)?;
        if global.mask.contains(&events.CALL) {
            register_callback(py, &global.tool, &events.CALL, None)?;
        }
        if global.mask.contains(&events.LINE) {
            register_callback(py, &global.tool, &events.LINE, None)?;
        }
        if global.mask.contains(&events.INSTRUCTION) {
            register_callback(py, &global.tool, &events.INSTRUCTION, None)?;
        }
        if global.mask.contains(&events.JUMP) {
            register_callback(py, &global.tool, &events.JUMP, None)?;
        }
        if global.mask.contains(&events.BRANCH) {
            register_callback(py, &global.tool, &events.BRANCH, None)?;
        }
        if global.mask.contains(&events.PY_START) {
            register_callback(py, &global.tool, &events.PY_START, None)?;
        }
        if global.mask.contains(&events.PY_RESUME) {
            register_callback(py, &global.tool, &events.PY_RESUME, None)?;
        }
        if global.mask.contains(&events.PY_RETURN) {
            register_callback(py, &global.tool, &events.PY_RETURN, None)?;
        }
        if global.mask.contains(&events.PY_YIELD) {
            register_callback(py, &global.tool, &events.PY_YIELD, None)?;
        }
        if global.mask.contains(&events.PY_THROW) {
            register_callback(py, &global.tool, &events.PY_THROW, None)?;
        }
        if global.mask.contains(&events.PY_UNWIND) {
            register_callback(py, &global.tool, &events.PY_UNWIND, None)?;
        }
        if global.mask.contains(&events.RAISE) {
            register_callback(py, &global.tool, &events.RAISE, None)?;
        }
        if global.mask.contains(&events.RERAISE) {
            register_callback(py, &global.tool, &events.RERAISE, None)?;
        }
        if global.mask.contains(&events.EXCEPTION_HANDLED) {
            register_callback(py, &global.tool, &events.EXCEPTION_HANDLED, None)?;
        }
        // if global.mask.contains(&events.STOP_ITERATION) {
        //     register_callback(py, &global.tool, &events.STOP_ITERATION, None)?;
        // }
        if global.mask.contains(&events.C_RETURN) {
            register_callback(py, &global.tool, &events.C_RETURN, None)?;
        }
        if global.mask.contains(&events.C_RAISE) {
            register_callback(py, &global.tool, &events.C_RAISE, None)?;
        }

        set_events(py, &global.tool, NO_EVENTS)?;
        free_tool_id(py, &global.tool)?;
    }
    Ok(())
}

/// Install a tracer and hook it into Python's `sys.monitoring`.
pub fn install_tracer(py: Python<'_>, tracer: Box<dyn Tracer>) -> PyResult<()> {
    let mut guard = GLOBAL.lock().unwrap();
    if guard.is_some() {
        return Err(ffi::map_recorder_error(usage!(
            ErrorCode::TracerInstallConflict,
            "tracer already installed"
        )));
    }

    let tool = acquire_tool_id(py)?;
    let events = monitoring_events(py)?;
    let monitoring = py.import("sys")?.getattr("monitoring")?;
    let disable_sentinel = monitoring.getattr("DISABLE")?.unbind();

    let module = PyModule::new(py, "_codetracer_callbacks")?;

    let mask = tracer.interest(events);

    if mask.contains(&events.CALL) {
        let cb = wrap_pyfunction!(callback_call, &module)?;
        register_callback(py, &tool, &events.CALL, Some(&cb))?;
    }
    if mask.contains(&events.LINE) {
        let cb = wrap_pyfunction!(callback_line, &module)?;
        register_callback(py, &tool, &events.LINE, Some(&cb))?;
    }
    if mask.contains(&events.INSTRUCTION) {
        let cb = wrap_pyfunction!(callback_instruction, &module)?;
        register_callback(py, &tool, &events.INSTRUCTION, Some(&cb))?;
    }
    if mask.contains(&events.JUMP) {
        let cb = wrap_pyfunction!(callback_jump, &module)?;
        register_callback(py, &tool, &events.JUMP, Some(&cb))?;
    }
    if mask.contains(&events.BRANCH) {
        let cb = wrap_pyfunction!(callback_branch, &module)?;
        register_callback(py, &tool, &events.BRANCH, Some(&cb))?;
    }
    if mask.contains(&events.PY_START) {
        let cb = wrap_pyfunction!(callback_py_start, &module)?;
        register_callback(py, &tool, &events.PY_START, Some(&cb))?;
    }
    if mask.contains(&events.PY_RESUME) {
        let cb = wrap_pyfunction!(callback_py_resume, &module)?;
        register_callback(py, &tool, &events.PY_RESUME, Some(&cb))?;
    }
    if mask.contains(&events.PY_RETURN) {
        let cb = wrap_pyfunction!(callback_py_return, &module)?;
        register_callback(py, &tool, &events.PY_RETURN, Some(&cb))?;
    }
    if mask.contains(&events.PY_YIELD) {
        let cb = wrap_pyfunction!(callback_py_yield, &module)?;
        register_callback(py, &tool, &events.PY_YIELD, Some(&cb))?;
    }
    if mask.contains(&events.PY_THROW) {
        let cb = wrap_pyfunction!(callback_py_throw, &module)?;
        register_callback(py, &tool, &events.PY_THROW, Some(&cb))?;
    }
    if mask.contains(&events.PY_UNWIND) {
        let cb = wrap_pyfunction!(callback_py_unwind, &module)?;
        register_callback(py, &tool, &events.PY_UNWIND, Some(&cb))?;
    }
    if mask.contains(&events.RAISE) {
        let cb = wrap_pyfunction!(callback_raise, &module)?;
        register_callback(py, &tool, &events.RAISE, Some(&cb))?;
    }
    if mask.contains(&events.RERAISE) {
        let cb = wrap_pyfunction!(callback_reraise, &module)?;
        register_callback(py, &tool, &events.RERAISE, Some(&cb))?;
    }
    if mask.contains(&events.EXCEPTION_HANDLED) {
        let cb = wrap_pyfunction!(callback_exception_handled, &module)?;
        register_callback(py, &tool, &events.EXCEPTION_HANDLED, Some(&cb))?;
    }
    // See comment in Tracer trait
    // if mask.contains(&events.STOP_ITERATION) {
    //     let cb = wrap_pyfunction!(callback_stop_iteration, &module)?;
    //     register_callback(py, &tool, &events.STOP_ITERATION, Some(&cb))?;
    // }
    if mask.contains(&events.C_RETURN) {
        let cb = wrap_pyfunction!(callback_c_return, &module)?;
        register_callback(py, &tool, &events.C_RETURN, Some(&cb))?;
    }
    if mask.contains(&events.C_RAISE) {
        let cb = wrap_pyfunction!(callback_c_raise, &module)?;
        register_callback(py, &tool, &events.C_RAISE, Some(&cb))?;
    }

    set_events(py, &tool, mask)?;

    *guard = Some(Global {
        registry: CodeObjectRegistry::default(),
        tracer,
        mask,
        tool,
        disable_sentinel,
    });
    Ok(())
}

/// Remove the installed tracer if any.
pub fn uninstall_tracer(py: Python<'_>) -> PyResult<()> {
    let mut guard = GLOBAL.lock().unwrap();
    uninstall_locked(py, &mut guard)
}

/// Flush the currently installed tracer if any.
pub fn flush_installed_tracer(py: Python<'_>) -> PyResult<()> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        global.tracer.flush(py)?;
    }
    Ok(())
}

#[pyfunction]
fn callback_call(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    offset: i32,
    callable: Bound<'_, PyAny>,
    arg0: Option<Bound<'_, PyAny>>,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_call", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_call(py, &wrapper, offset, &callable, arg0.as_ref())
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_line(py: Python<'_>, code: Bound<'_, PyCode>, lineno: u32) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_line", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global.tracer.on_line(py, &wrapper, lineno)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_instruction(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_instruction", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_instruction(py, &wrapper, instruction_offset)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_jump(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    destination_offset: i32,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_jump", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_jump(py, &wrapper, instruction_offset, destination_offset)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_branch(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    destination_offset: i32,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_branch", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_branch(py, &wrapper, instruction_offset, destination_offset)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_py_start(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_py_start", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global.tracer.on_py_start(py, &wrapper, instruction_offset)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_py_resume(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_py_resume", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global.tracer.on_py_resume(py, &wrapper, instruction_offset)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_py_return(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    retval: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_py_return", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_py_return(py, &wrapper, instruction_offset, &retval)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_py_yield(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    retval: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_py_yield", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_py_yield(py, &wrapper, instruction_offset, &retval)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_py_throw(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    exception: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_py_throw", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_py_throw(py, &wrapper, instruction_offset, &exception)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_py_unwind(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    exception: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_py_unwind", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_py_unwind(py, &wrapper, instruction_offset, &exception)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_raise(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    exception: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_raise", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_raise(py, &wrapper, instruction_offset, &exception)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_reraise(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    exception: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_reraise", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_reraise(py, &wrapper, instruction_offset, &exception)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_exception_handled(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    exception: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_exception_handled", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_exception_handled(py, &wrapper, instruction_offset, &exception)
        };
        handle_callback_result(py, &mut guard, result)
    })
}

// See comment in Tracer trait
// #[pyfunction]
// fn callback_stop_iteration(
//     py: Python<'_>,
//     code: Bound<'_, PyAny>,
//     instruction_offset: i32,
//     exception: Bound<'_, PyAny>,
// ) -> PyResult<()> {
//     if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
//         global
//             .tracer
//             .on_stop_iteration(py, &code, instruction_offset, &exception);
//     }
//     Ok(())
// }

#[pyfunction]
fn callback_c_return(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    offset: i32,
    callable: Bound<'_, PyAny>,
    arg0: Option<Bound<'_, PyAny>>,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_c_return", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_c_return(py, &wrapper, offset, &callable, arg0.as_ref())
        };
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
fn callback_c_raise(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    offset: i32,
    callable: Bound<'_, PyAny>,
    arg0: Option<Bound<'_, PyAny>>,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_c_raise", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = {
            let global = guard.as_mut().expect("tracer installed");
            let wrapper = global.registry.get_or_insert(py, &code);
            global
                .tracer
                .on_c_raise(py, &wrapper, offset, &callable, arg0.as_ref())
        };
        handle_callback_result(py, &mut guard, result)
    })
}
