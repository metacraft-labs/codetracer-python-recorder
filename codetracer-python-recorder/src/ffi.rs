//! FFI helpers bridging `RecorderError` into Python exceptions with panic containment.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use recorder_errors::{ErrorCode, ErrorKind, RecorderError, RecorderResult};

use crate::logging;

create_exception!(codetracer_python_recorder, PyRecorderError, PyException);
create_exception!(codetracer_python_recorder, PyUsageError, PyRecorderError);
create_exception!(
    codetracer_python_recorder,
    PyEnvironmentError,
    PyRecorderError
);
create_exception!(codetracer_python_recorder, PyTargetError, PyRecorderError);
create_exception!(codetracer_python_recorder, PyInternalError, PyRecorderError);

/// Register the recorder exception hierarchy into the Python module.
pub fn register_exceptions(module: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = module.py();
    module.add("RecorderError", py.get_type::<PyRecorderError>())?;
    module.add("UsageError", py.get_type::<PyUsageError>())?;
    module.add("EnvironmentError", py.get_type::<PyEnvironmentError>())?;
    module.add("TargetError", py.get_type::<PyTargetError>())?;
    module.add("InternalError", py.get_type::<PyInternalError>())?;
    Ok(())
}

/// Execute `operation`, mapping any `RecorderError` into the Python exception hierarchy
/// and containing panics as `PyInternalError` instances.
#[allow(dead_code)]
pub fn dispatch<T, F>(label: &'static str, operation: F) -> PyResult<T>
where
    F: FnOnce() -> RecorderResult<T>,
{
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => result.map_err(map_recorder_error),
        Err(panic_payload) => Err(handle_panic(label, panic_payload)),
    }
}

/// Convert a captured panic into a `PyInternalError` while logging the payload.
pub(crate) fn panic_to_pyerr(label: &'static str, payload: Box<dyn Any + Send>) -> PyErr {
    handle_panic(label, payload)
}

fn handle_panic(label: &'static str, payload: Box<dyn Any + Send>) -> PyErr {
    let message = panic_payload_to_string(&payload);
    logging::record_panic(label);
    map_recorder_error(RecorderError::new(
        ErrorKind::Internal,
        ErrorCode::Unknown,
        format!("panic in {label}: {message}"),
    ))
}

fn panic_payload_to_string(payload: &Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        message.to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Map a `RecorderError` into the appropriate Python exception subclass.
pub fn map_recorder_error(err: RecorderError) -> PyErr {
    logging::log_recorder_error("recorder_error", &err);
    logging::emit_error_trailer(&err);
    let source_desc = err.source_ref().map(|src| src.to_string());
    let RecorderError {
        kind,
        code,
        message,
        context,
        ..
    } = err;

    let mut text = format!("[{code}] {message}");
    if !context.is_empty() {
        let mut first = true;
        text.push_str(" (");
        for (key, value) in &context {
            if !first {
                text.push_str(", ");
            }
            first = false;
            text.push_str(key);
            text.push('=');
            text.push_str(value);
        }
        text.push(')');
    }
    if let Some(source) = source_desc.as_ref() {
        text.push_str(": caused by ");
        text.push_str(source);
    }

    let pyerr = match kind {
        ErrorKind::Usage => PyUsageError::new_err(text.clone()),
        ErrorKind::Environment => PyEnvironmentError::new_err(text.clone()),
        ErrorKind::Target => PyTargetError::new_err(text.clone()),
        ErrorKind::Internal => PyInternalError::new_err(text.clone()),
        _ => PyInternalError::new_err(text.clone()),
    };

    Python::with_gil(|py| {
        let instance = pyerr.value(py);
        let _ = instance.setattr("code", code.as_str());
        let _ = instance.setattr("kind", format!("{:?}", kind));
        let context_dict = PyDict::new(py);
        for (key, value) in &context {
            let _ = context_dict.set_item(*key, value);
        }
        let _ = instance.setattr("context", context_dict);
    });

    pyerr
}

/// Helper that guards a `#[pyfunction]` implementation, catching panics while
/// leaving existing `PyResult` usage intact.
pub fn wrap_pyfunction<T, F>(label: &'static str, operation: F) -> PyResult<T>
where
    F: FnOnce() -> PyResult<T>,
{
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(panic_payload) => Err(handle_panic(label, panic_payload)),
    }
}
