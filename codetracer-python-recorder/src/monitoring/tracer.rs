//! Tracer trait and sys.monitoring callback plumbing.

use std::any::Any;
use std::collections::HashSet;
use std::sync::Mutex;

use crate::code_object::{CodeObjectRegistry, CodeObjectWrapper};
use pyo3::{
    exceptions::PyRuntimeError,
    prelude::*,
    types::{PyAny, PyCode, PyModule, PyTuple},
    ToPyObject,
};

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
    backend: Backend,
}

enum Backend {
    Monitoring {
        tool: ToolId,
        disable_sentinel: Py<PyAny>,
    },
    Legacy(LegacyState),
}

struct LegacyState {
    events: MonitoringEvents,
    global_trace: Py<PyAny>,
    local_trace: Py<PyAny>,
    disabled_codes: HashSet<usize>,
}

static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);

impl Global {
    fn handle_monitoring_callback(
        &self,
        py: Python<'_>,
        result: CallbackResult,
    ) -> PyResult<Py<PyAny>> {
        let Backend::Monitoring {
            disable_sentinel, ..
        } = &self.backend
        else {
            // Legacy callbacks do not use monitoring sentinels.
            return Ok(py.None());
        };

        match result? {
            CallbackOutcome::Continue => Ok(py.None()),
            CallbackOutcome::DisableLocation => Ok(disable_sentinel.clone_ref(py)),
        }
    }
}

/// Install a tracer and hook it into Python's tracing facilities.
pub fn install_tracer(py: Python<'_>, tracer: Box<dyn Tracer>) -> PyResult<()> {
    if has_sys_monitoring(py) {
        install_with_monitoring(py, tracer)
    } else {
        install_with_legacy_tracing(py, tracer)
    }
}

fn has_sys_monitoring(py: Python<'_>) -> bool {
    py.import("sys")
        .and_then(|sys| sys.getattr("monitoring"))
        .map(|monitoring| monitoring.hasattr("use_tool_id").unwrap_or(false))
        .unwrap_or(false)
}

fn install_with_monitoring(py: Python<'_>, tracer: Box<dyn Tracer>) -> PyResult<()> {
    let mut guard = GLOBAL.lock().unwrap();
    if guard.is_some() {
        return Err(PyRuntimeError::new_err("tracer already installed"));
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
        backend: Backend::Monitoring {
            tool,
            disable_sentinel,
        },
    });
    Ok(())
}

fn install_with_legacy_tracing(py: Python<'_>, tracer: Box<dyn Tracer>) -> PyResult<()> {
    let mut guard = GLOBAL.lock().unwrap();
    if guard.is_some() {
        return Err(PyRuntimeError::new_err("tracer already installed"));
    }

    let events = legacy_monitoring_events();
    let mask = tracer.interest(&events);

    let module = PyModule::new(py, "_codetracer_legacy_callbacks")?;
    let global_cb = wrap_pyfunction!(legacy_global_trace, &module)?;
    let local_cb = wrap_pyfunction!(legacy_local_trace, &module)?;

    let global_trace = global_cb.unbind();
    let local_trace = local_cb.unbind();

    *guard = Some(Global {
        registry: CodeObjectRegistry::default(),
        tracer,
        mask,
        backend: Backend::Legacy(LegacyState {
            events,
            global_trace: global_trace.clone_ref(py),
            local_trace: local_trace.clone_ref(py),
            disabled_codes: HashSet::new(),
        }),
    });
    drop(guard);

    let sys = py.import("sys")?;
    sys.call_method1("settrace", (global_trace.bind(py),))?;
    Ok(())
}

fn legacy_monitoring_events() -> MonitoringEvents {
    fn bit(n: i32) -> i32 {
        1 << n
    }

    MonitoringEvents {
        BRANCH: EventId(bit(0)),
        CALL: EventId(bit(1)),
        C_RAISE: EventId(bit(2)),
        C_RETURN: EventId(bit(3)),
        EXCEPTION_HANDLED: EventId(bit(4)),
        INSTRUCTION: EventId(bit(5)),
        JUMP: EventId(bit(6)),
        LINE: EventId(bit(7)),
        PY_RESUME: EventId(bit(8)),
        PY_RETURN: EventId(bit(9)),
        PY_START: EventId(bit(10)),
        PY_THROW: EventId(bit(11)),
        PY_UNWIND: EventId(bit(12)),
        PY_YIELD: EventId(bit(13)),
        RAISE: EventId(bit(14)),
        RERAISE: EventId(bit(15)),
    }
}

/// Remove the installed tracer if any.
pub fn uninstall_tracer(py: Python<'_>) -> PyResult<()> {
    let mut guard = GLOBAL.lock().unwrap();
    if let Some(mut global) = guard.take() {
        // Give the tracer a chance to finish underlying writers before
        // unregistering callbacks or removing tracing hooks.
        let _ = global.tracer.finish(py);
        match &global.backend {
            Backend::Monitoring { tool, .. } => {
                let events = monitoring_events(py)?;
                if global.mask.contains(&events.CALL) {
                    register_callback(py, tool, &events.CALL, None)?;
                }
                if global.mask.contains(&events.LINE) {
                    register_callback(py, tool, &events.LINE, None)?;
                }
                if global.mask.contains(&events.INSTRUCTION) {
                    register_callback(py, tool, &events.INSTRUCTION, None)?;
                }
                if global.mask.contains(&events.JUMP) {
                    register_callback(py, tool, &events.JUMP, None)?;
                }
                if global.mask.contains(&events.BRANCH) {
                    register_callback(py, tool, &events.BRANCH, None)?;
                }
                if global.mask.contains(&events.PY_START) {
                    register_callback(py, tool, &events.PY_START, None)?;
                }
                if global.mask.contains(&events.PY_RESUME) {
                    register_callback(py, tool, &events.PY_RESUME, None)?;
                }
                if global.mask.contains(&events.PY_RETURN) {
                    register_callback(py, tool, &events.PY_RETURN, None)?;
                }
                if global.mask.contains(&events.PY_YIELD) {
                    register_callback(py, tool, &events.PY_YIELD, None)?;
                }
                if global.mask.contains(&events.PY_THROW) {
                    register_callback(py, tool, &events.PY_THROW, None)?;
                }
                if global.mask.contains(&events.PY_UNWIND) {
                    register_callback(py, tool, &events.PY_UNWIND, None)?;
                }
                if global.mask.contains(&events.RAISE) {
                    register_callback(py, tool, &events.RAISE, None)?;
                }
                if global.mask.contains(&events.RERAISE) {
                    register_callback(py, tool, &events.RERAISE, None)?;
                }
                if global.mask.contains(&events.EXCEPTION_HANDLED) {
                    register_callback(py, tool, &events.EXCEPTION_HANDLED, None)?;
                }
                if global.mask.contains(&events.C_RETURN) {
                    register_callback(py, tool, &events.C_RETURN, None)?;
                }
                if global.mask.contains(&events.C_RAISE) {
                    register_callback(py, tool, &events.C_RAISE, None)?;
                }

                set_events(py, tool, NO_EVENTS)?;
                free_tool_id(py, tool)?;
            }
            Backend::Legacy(_) => {
                let sys = py.import("sys")?;
                sys.call_method1("settrace", (py.None(),))?;
            }
        }
    }
    Ok(())
}

/// Flush the currently installed tracer if any.
pub fn flush_installed_tracer(py: Python<'_>) -> PyResult<()> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        global.tracer.flush(py)?;
        if let Backend::Legacy(state) = &mut global.backend {
            state.disabled_codes.clear();
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegacyDirective {
    Continue,
    StopTracing,
}

fn apply_legacy_result(
    state: &mut LegacyState,
    code_id: usize,
    result: CallbackResult,
) -> PyResult<bool> {
    match result? {
        CallbackOutcome::Continue => Ok(true),
        CallbackOutcome::DisableLocation => {
            state.disabled_codes.insert(code_id);
            Ok(false)
        }
    }
}

fn handle_legacy_event(
    py: Python<'_>,
    frame: &Bound<'_, PyAny>,
    event: &str,
    arg: Option<&Bound<'_, PyAny>>,
) -> PyResult<LegacyDirective> {
    let mut guard = GLOBAL.lock().unwrap();
    let Some(global) = guard.as_mut() else {
        return Ok(LegacyDirective::StopTracing);
    };
    let Backend::Legacy(state) = &mut global.backend else {
        return Ok(LegacyDirective::StopTracing);
    };

    let code_obj = frame.getattr("f_code")?;
    let code_obj = code_obj.downcast::<PyCode>()?;
    let wrapper = global.registry.get_or_insert(py, &code_obj);
    let code_id = wrapper.id();

    if state.disabled_codes.contains(&code_id) {
        return Ok(LegacyDirective::StopTracing);
    }

    let offset: i32 = frame.getattr("f_lasti")?.extract()?;

    match event {
        "call" => {
            let mut keep_tracing = true;
            if global.mask.contains(&state.events.CALL) {
                let callable_obj = frame.getattr("f_code")?.to_object(py);
                let callable_bound = callable_obj.bind(py);
                keep_tracing &= apply_legacy_result(
                    state,
                    code_id,
                    global
                        .tracer
                        .on_call(py, &wrapper, offset, &callable_bound, None),
                )?;
            }
            if keep_tracing && global.mask.contains(&state.events.PY_START) {
                keep_tracing &= apply_legacy_result(
                    state,
                    code_id,
                    global.tracer.on_py_start(py, &wrapper, offset),
                )?;
            }
            Ok(if keep_tracing {
                LegacyDirective::Continue
            } else {
                LegacyDirective::StopTracing
            })
        }
        "line" => {
            if !global.mask.contains(&state.events.LINE) {
                return Ok(LegacyDirective::Continue);
            }
            let lineno = frame.getattr("f_lineno")?.extract()?;
            let keep_tracing =
                apply_legacy_result(state, code_id, global.tracer.on_line(py, &wrapper, lineno))?;
            Ok(if keep_tracing {
                LegacyDirective::Continue
            } else {
                LegacyDirective::StopTracing
            })
        }
        "return" => {
            if !global.mask.contains(&state.events.PY_RETURN)
                && !global.mask.contains(&state.events.PY_YIELD)
            {
                return Ok(LegacyDirective::Continue);
            }

            let retval_obj = arg
                .map(|value| value.to_object(py))
                .unwrap_or_else(|| py.None());
            let retval_bound = retval_obj.bind(py);

            const CO_GENERATOR: u32 = 0x20;
            const CO_COROUTINE: u32 = 0x80;
            const CO_ASYNC_GENERATOR: u32 = 0x200;
            let flags: u32 = code_obj.getattr("co_flags")?.extract()?;
            let is_generator_like =
                (flags & (CO_GENERATOR | CO_COROUTINE | CO_ASYNC_GENERATOR)) != 0;

            let mut keep_tracing = true;
            if is_generator_like && global.mask.contains(&state.events.PY_YIELD) {
                keep_tracing &= apply_legacy_result(
                    state,
                    code_id,
                    global
                        .tracer
                        .on_py_yield(py, &wrapper, offset, &retval_bound),
                )?;
            }

            if keep_tracing && global.mask.contains(&state.events.PY_RETURN) {
                keep_tracing &= apply_legacy_result(
                    state,
                    code_id,
                    global
                        .tracer
                        .on_py_return(py, &wrapper, offset, &retval_bound),
                )?;
            }

            Ok(if keep_tracing {
                LegacyDirective::Continue
            } else {
                LegacyDirective::StopTracing
            })
        }
        "exception" => {
            if !global.mask.contains(&state.events.RAISE) {
                return Ok(LegacyDirective::Continue);
            }
            if let Some(exc_info) = arg {
                let exc_obj = if let Ok(tuple) = exc_info.downcast::<PyTuple>() {
                    if tuple.len() >= 2 {
                        tuple.get_item(1).to_object(py)
                    } else {
                        exc_info.to_object(py)
                    }
                } else {
                    exc_info.to_object(py)
                };
                let exc_bound = exc_obj.bind(py);
                let keep_tracing = apply_legacy_result(
                    state,
                    code_id,
                    global.tracer.on_raise(py, &wrapper, offset, &exc_bound),
                )?;
                return Ok(if keep_tracing {
                    LegacyDirective::Continue
                } else {
                    LegacyDirective::StopTracing
                });
            }
            Ok(LegacyDirective::Continue)
        }
        "c_return" => {
            if !global.mask.contains(&state.events.C_RETURN) {
                return Ok(LegacyDirective::Continue);
            }
            if let Some(callable) = arg {
                let callable_obj = callable.to_object(py);
                let callable_bound = callable_obj.bind(py);
                let keep_tracing = apply_legacy_result(
                    state,
                    code_id,
                    global
                        .tracer
                        .on_c_return(py, &wrapper, offset, &callable_bound, None),
                )?;
                return Ok(if keep_tracing {
                    LegacyDirective::Continue
                } else {
                    LegacyDirective::StopTracing
                });
            }
            Ok(LegacyDirective::Continue)
        }
        "c_exception" => {
            if !global.mask.contains(&state.events.C_RAISE) {
                return Ok(LegacyDirective::Continue);
            }
            if let Some(callable) = arg {
                let callable_obj = callable.to_object(py);
                let callable_bound = callable_obj.bind(py);
                let keep_tracing = apply_legacy_result(
                    state,
                    code_id,
                    global
                        .tracer
                        .on_c_raise(py, &wrapper, offset, &callable_bound, None),
                )?;
                return Ok(if keep_tracing {
                    LegacyDirective::Continue
                } else {
                    LegacyDirective::StopTracing
                });
            }
            Ok(LegacyDirective::Continue)
        }
        _ => Ok(LegacyDirective::Continue),
    }
}

fn legacy_trace_impl(
    py: Python<'_>,
    frame: Bound<'_, PyAny>,
    event: &str,
    arg: Option<Bound<'_, PyAny>>,
) -> PyResult<Py<PyAny>> {
    let directive = handle_legacy_event(py, &frame, event, arg.as_ref())?;
    match directive {
        LegacyDirective::StopTracing => Ok(py.None()),
        LegacyDirective::Continue => {
            let guard = GLOBAL.lock().unwrap();
            if let Some(global) = guard.as_ref() {
                if let Backend::Legacy(state) = &global.backend {
                    return Ok(state.local_trace.clone_ref(py));
                }
            }
            Ok(py.None())
        }
    }
}

#[pyfunction]
fn legacy_global_trace(
    py: Python<'_>,
    frame: Bound<'_, PyAny>,
    event: &str,
    arg: Option<Bound<'_, PyAny>>,
) -> PyResult<Py<PyAny>> {
    legacy_trace_impl(py, frame, event, arg)
}

#[pyfunction]
fn legacy_local_trace(
    py: Python<'_>,
    frame: Bound<'_, PyAny>,
    event: &str,
    arg: Option<Bound<'_, PyAny>>,
) -> PyResult<Py<PyAny>> {
    legacy_trace_impl(py, frame, event, arg)
}

#[pyfunction]
fn callback_call(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    offset: i32,
    callable: Bound<'_, PyAny>,
    arg0: Option<Bound<'_, PyAny>>,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_call(py, &wrapper, offset, &callable, arg0.as_ref());
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_line(py: Python<'_>, code: Bound<'_, PyCode>, lineno: u32) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global.tracer.on_line(py, &wrapper, lineno);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_instruction(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_instruction(py, &wrapper, instruction_offset);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_jump(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    destination_offset: i32,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_jump(py, &wrapper, instruction_offset, destination_offset);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_branch(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    destination_offset: i32,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_branch(py, &wrapper, instruction_offset, destination_offset);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_py_start(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global.tracer.on_py_start(py, &wrapper, instruction_offset);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_py_resume(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global.tracer.on_py_resume(py, &wrapper, instruction_offset);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_py_return(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    retval: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_py_return(py, &wrapper, instruction_offset, &retval);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_py_yield(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    retval: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_py_yield(py, &wrapper, instruction_offset, &retval);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_py_throw(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    exception: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_py_throw(py, &wrapper, instruction_offset, &exception);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_py_unwind(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    exception: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_py_unwind(py, &wrapper, instruction_offset, &exception);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_raise(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    exception: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_raise(py, &wrapper, instruction_offset, &exception);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_reraise(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    exception: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_reraise(py, &wrapper, instruction_offset, &exception);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_exception_handled(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
    exception: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result =
            global
                .tracer
                .on_exception_handled(py, &wrapper, instruction_offset, &exception);
        return global.handle_monitoring_callback(py, result);
    }
    Ok(py.None())
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
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_c_return(py, &wrapper, offset, &callable, arg0.as_ref());
        return global.handle_callback(py, result);
    }
    Ok(py.None())
}

#[pyfunction]
fn callback_c_raise(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    offset: i32,
    callable: Bound<'_, PyAny>,
    arg0: Option<Bound<'_, PyAny>>,
) -> PyResult<Py<PyAny>> {
    if let Some(global) = GLOBAL.lock().unwrap().as_mut() {
        let wrapper = global.registry.get_or_insert(py, &code);
        let result = global
            .tracer
            .on_c_raise(py, &wrapper, offset, &callable, arg0.as_ref());
        return global.handle_callback(py, result);
    }
    Ok(py.None())
}
