//! sys.monitoring callback plumbing and tracer installation.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use crate::code_object::{CodeObjectRegistry, CodeObjectWrapper};
use crate::ffi;
use crate::logging;
use crate::policy::{self, OnRecorderError};
use log::{error, warn};
use pyo3::{
    prelude::*,
    types::{PyAny, PyCode, PyModule},
};
use recorder_errors::{usage, ErrorCode};

use super::api::Tracer;
use super::callbacks::{CallbackOutcome, CallbackResult};
use super::{
    acquire_tool_id, free_tool_id, monitoring_events, register_callback, set_events, EventSet,
    ToolId, NO_EVENTS,
};

struct Global {
    registry: CodeObjectRegistry,
    tracer: Box<dyn Tracer>,
    mask: EventSet,
    tool: ToolId,
    disable_sentinel: Py<PyAny>,
}

static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);

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
            uninstall_locked(py, guard)?;
            Ok(py.None())
        }
    }
}

fn uninstall_locked(py: Python<'_>, guard: &mut Option<Global>) -> PyResult<()> {
    if let Some(mut global) = guard.take() {
        let finish_result = global.tracer.finish(py);

        let cleanup_result = (|| -> PyResult<()> {
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
            Ok(())
        })();

        if let Err(err) = finish_result {
            if let Err(cleanup_err) = cleanup_result {
                warn!(
                    "failed to reset monitoring callbacks after finish error: {}",
                    cleanup_err
                );
            }
            return Err(err);
        }

        cleanup_result?;
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
        let result =
            call_tracer_with_code(py, &mut guard, &code, "callback_call", |tracer, wrapper| {
                tracer.on_call(py, wrapper, offset, &callable, arg0.as_ref())
            });
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
        let result =
            call_tracer_with_code(py, &mut guard, &code, "callback_line", |tracer, wrapper| {
                tracer.on_line(py, wrapper, lineno)
            });
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
        let result =
            call_tracer_with_code(py, &mut guard, &code, "callback_jump", |tracer, wrapper| {
                tracer.on_jump(py, wrapper, instruction_offset, destination_offset)
            });
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
