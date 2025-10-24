//! sys.monitoring callback metadata and helpers.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use crate::code_object::{CodeObjectRegistry, CodeObjectWrapper};
use crate::ffi;
use crate::logging;
use crate::policy::{self, OnRecorderError};
use log::{error, trace, warn};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyCode, PyModule};
use pyo3::wrap_pyfunction;
use recorder_errors::ErrorCode;

use super::api::Tracer;
use super::{register_callback, EventId, EventSet, MonitoringEvents, ToolId};

pub use super::{CallbackFn, CallbackOutcome, CallbackResult};

/// Global tracer state shared between callback invocations and installer.
pub(super) struct Global {
    pub(super) registry: CodeObjectRegistry,
    pub(super) tracer: Box<dyn Tracer>,
    pub(super) mask: EventSet,
    pub(super) tool: ToolId,
    pub(super) disable_sentinel: Py<PyAny>,
}

pub(super) static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);

fn catch_callback<F>(label: &'static str, callback: F) -> CallbackResult
where
    F: FnOnce() -> CallbackResult,
{
    match catch_unwind(AssertUnwindSafe(callback)) {
        Ok(result) => result,
        Err(payload) => Err(ffi::panic_to_pyerr(label, payload)),
    }
}

fn call_tracer_with_code<'py, F>(
    py: Python<'py>,
    guard: &mut Option<Global>,
    code: &Bound<'py, PyCode>,
    label: &'static str,
    callback: F,
) -> CallbackResult
where
    F: FnOnce(&mut dyn Tracer, &CodeObjectWrapper) -> CallbackResult,
{
    let global = guard.as_mut().expect("tracer installed");
    let wrapper = global.registry.get_or_insert(py, code);
    let tracer = global.tracer.as_mut();
    catch_callback(label, || callback(tracer, &wrapper))
}

fn handle_callback_result(
    py: Python<'_>,
    guard: &mut Option<Global>,
    result: CallbackResult,
) -> PyResult<Py<PyAny>> {
    match result {
        Ok(CallbackOutcome::Continue) => Ok(py.None()),
        Ok(CallbackOutcome::DisableLocation) => Ok(guard
            .as_ref()
            .map(|global| global.disable_sentinel.clone_ref(py))
            .unwrap_or_else(|| py.None())),
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
            let code = logging::error_code_from_pyerr(py, &err);
            logging::record_detach("policy_disable", code.map(|code| code.as_str()));
            logging::with_error_code_opt(code, || {
                error!(
                    "recorder callback error; disabling tracer per policy: {}",
                    message
                );
            });
            if let Some(global) = guard.as_mut() {
                if let Err(notify_err) = global.tracer.notify_failure(py) {
                    logging::with_error_code(ErrorCode::TraceIncomplete, || {
                        warn!(
                            "failed to notify tracer about disable transition: {}",
                            notify_err
                        );
                    });
                }
            }
            super::install::uninstall_locked(py, guard)?;
            Ok(py.None())
        }
    }
}

#[pyfunction]
pub(super) fn callback_call(
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
        let result =
            call_tracer_with_code(py, &mut guard, &code, "callback_call", |tracer, wrapper| {
                tracer.on_call(py, wrapper, offset, &callable, arg0.as_ref())
            });
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_line(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    lineno: u32,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_line", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result =
            call_tracer_with_code(py, &mut guard, &code, "callback_line", |tracer, wrapper| {
                tracer.on_line(py, wrapper, lineno)
            });
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_instruction(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_instruction", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_instruction",
            |tracer, wrapper| tracer.on_instruction(py, wrapper, instruction_offset),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_jump(
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
        let result =
            call_tracer_with_code(py, &mut guard, &code, "callback_jump", |tracer, wrapper| {
                tracer.on_jump(py, wrapper, instruction_offset, destination_offset)
            });
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_branch(
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
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_branch",
            |tracer, wrapper| tracer.on_branch(py, wrapper, instruction_offset, destination_offset),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_py_start(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_py_start", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_py_start",
            |tracer, wrapper| tracer.on_py_start(py, wrapper, instruction_offset),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_py_resume(
    py: Python<'_>,
    code: Bound<'_, PyCode>,
    instruction_offset: i32,
) -> PyResult<Py<PyAny>> {
    ffi::wrap_pyfunction("callback_py_resume", || {
        let mut guard = GLOBAL.lock().unwrap();
        if guard.is_none() {
            return Ok(py.None());
        }
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_py_resume",
            |tracer, wrapper| tracer.on_py_resume(py, wrapper, instruction_offset),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_py_return(
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
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_py_return",
            |tracer, wrapper| tracer.on_py_return(py, wrapper, instruction_offset, &retval),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_py_yield(
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
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_py_yield",
            |tracer, wrapper| tracer.on_py_yield(py, wrapper, instruction_offset, &retval),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_py_throw(
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
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_py_throw",
            |tracer, wrapper| tracer.on_py_throw(py, wrapper, instruction_offset, &exception),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_py_unwind(
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
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_py_unwind",
            |tracer, wrapper| tracer.on_py_unwind(py, wrapper, instruction_offset, &exception),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_raise(
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
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_raise",
            |tracer, wrapper| tracer.on_raise(py, wrapper, instruction_offset, &exception),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_reraise(
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
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_reraise",
            |tracer, wrapper| tracer.on_reraise(py, wrapper, instruction_offset, &exception),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_exception_handled(
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
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_exception_handled",
            |tracer, wrapper| {
                tracer.on_exception_handled(py, wrapper, instruction_offset, &exception)
            },
        );
        handle_callback_result(py, &mut guard, result)
    })
}

// See comment in Tracer trait
// #[pyfunction]
// pub(super) fn callback_stop_iteration(
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
pub(super) fn callback_c_return(
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
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_c_return",
            |tracer, wrapper| tracer.on_c_return(py, wrapper, offset, &callable, arg0.as_ref()),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

#[pyfunction]
pub(super) fn callback_c_raise(
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
        let result = call_tracer_with_code(
            py,
            &mut guard,
            &code,
            "callback_c_raise",
            |tracer, wrapper| tracer.on_c_raise(py, wrapper, offset, &callable, arg0.as_ref()),
        );
        handle_callback_result(py, &mut guard, result)
    })
}

/// Function pointer used to instantiate a PyO3 callback.
type CallbackFactory = for<'py> fn(&Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>>;

/// Metadata describing how to register a sys.monitoring callback.
pub struct CallbackSpec {
    /// Debug label (mirrors the PyO3 function name).
    pub name: &'static str,
    event: fn(&MonitoringEvents) -> EventId,
    factory: CallbackFactory,
}

impl CallbackSpec {
    pub const fn new(
        name: &'static str,
        event: fn(&MonitoringEvents) -> EventId,
        factory: CallbackFactory,
    ) -> Self {
        Self {
            name,
            event,
            factory,
        }
    }

    /// Resolve the CPython event identifier for this callback.
    pub fn event(&self, events: &MonitoringEvents) -> EventId {
        (self.event)(events)
    }

    /// Instantiate and bind the PyO3 callback into the provided module.
    pub fn make<'py>(&self, module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
        (self.factory)(module)
    }

    /// Return true when the callback should be active for the supplied mask.
    pub fn enabled(&self, mask: &EventSet, events: &MonitoringEvents) -> bool {
        mask.contains(&self.event(events))
    }
}

/// Declarative list describing all recorder callbacks.
pub static CALLBACK_SPECS: &[CallbackSpec] = &[
    CallbackSpec::new("callback_call", |ev| ev.CALL, wrap_callback_call),
    CallbackSpec::new("callback_line", |ev| ev.LINE, wrap_callback_line),
    CallbackSpec::new(
        "callback_instruction",
        |ev| ev.INSTRUCTION,
        wrap_callback_instruction,
    ),
    CallbackSpec::new("callback_jump", |ev| ev.JUMP, wrap_callback_jump),
    CallbackSpec::new("callback_branch", |ev| ev.BRANCH, wrap_callback_branch),
    CallbackSpec::new(
        "callback_py_start",
        |ev| ev.PY_START,
        wrap_callback_py_start,
    ),
    CallbackSpec::new(
        "callback_py_resume",
        |ev| ev.PY_RESUME,
        wrap_callback_py_resume,
    ),
    CallbackSpec::new(
        "callback_py_return",
        |ev| ev.PY_RETURN,
        wrap_callback_py_return,
    ),
    CallbackSpec::new(
        "callback_py_yield",
        |ev| ev.PY_YIELD,
        wrap_callback_py_yield,
    ),
    CallbackSpec::new(
        "callback_py_throw",
        |ev| ev.PY_THROW,
        wrap_callback_py_throw,
    ),
    CallbackSpec::new(
        "callback_py_unwind",
        |ev| ev.PY_UNWIND,
        wrap_callback_py_unwind,
    ),
    CallbackSpec::new("callback_raise", |ev| ev.RAISE, wrap_callback_raise),
    CallbackSpec::new("callback_reraise", |ev| ev.RERAISE, wrap_callback_reraise),
    CallbackSpec::new(
        "callback_exception_handled",
        |ev| ev.EXCEPTION_HANDLED,
        wrap_callback_exception_handled,
    ),
    // See comment in Tracer trait: STOP_ITERATION intentionally omitted.
    CallbackSpec::new(
        "callback_c_return",
        |ev| ev.C_RETURN,
        wrap_callback_c_return,
    ),
    CallbackSpec::new("callback_c_raise", |ev| ev.C_RAISE, wrap_callback_c_raise),
];

/// Iterate over the callbacks enabled for the provided mask.
pub fn enabled_specs<'a>(
    mask: &'a EventSet,
    events: &'a MonitoringEvents,
) -> impl Iterator<Item = &'static CallbackSpec> + 'a {
    CALLBACK_SPECS
        .iter()
        .filter(move |spec| spec.enabled(mask, events))
}

/// Register all callbacks enabled by the supplied mask.
pub fn register_enabled_callbacks<'py>(
    py: Python<'py>,
    module: &Bound<'py, PyModule>,
    tool: &ToolId,
    mask: &EventSet,
    events: &MonitoringEvents,
) -> PyResult<()> {
    for spec in enabled_specs(mask, events) {
        let event = spec.event(events);
        trace!(
            "[monitoring] registering callback `{}` for event id {}",
            spec.name,
            event.0
        );
        let cb = spec.make(module)?;
        register_callback(py, tool, &event, Some(&cb))?;
    }
    Ok(())
}

/// Unregister previously installed callbacks that were enabled by the mask.
pub fn unregister_enabled_callbacks(
    py: Python<'_>,
    tool: &ToolId,
    mask: &EventSet,
    events: &MonitoringEvents,
) -> PyResult<()> {
    for spec in enabled_specs(mask, events) {
        let event = spec.event(events);
        trace!(
            "[monitoring] unregistering callback `{}` for event id {}",
            spec.name,
            event.0
        );
        register_callback(py, tool, &event, None)?;
    }
    Ok(())
}

fn wrap_callback_call<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_call, module)
}

fn wrap_callback_line<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_line, module)
}

fn wrap_callback_instruction<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_instruction, module)
}

fn wrap_callback_jump<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_jump, module)
}

fn wrap_callback_branch<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_branch, module)
}

fn wrap_callback_py_start<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_py_start, module)
}

fn wrap_callback_py_resume<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_py_resume, module)
}

fn wrap_callback_py_return<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_py_return, module)
}

fn wrap_callback_py_yield<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_py_yield, module)
}

fn wrap_callback_py_throw<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_py_throw, module)
}

fn wrap_callback_py_unwind<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_py_unwind, module)
}

fn wrap_callback_raise<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_raise, module)
}

fn wrap_callback_reraise<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_reraise, module)
}

fn wrap_callback_exception_handled<'py>(
    module: &Bound<'py, PyModule>,
) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_exception_handled, module)
}

fn wrap_callback_c_return<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_c_return, module)
}

fn wrap_callback_c_raise<'py>(module: &Bound<'py, PyModule>) -> PyResult<CallbackFn<'py>> {
    wrap_pyfunction!(callback_c_raise, module)
}
