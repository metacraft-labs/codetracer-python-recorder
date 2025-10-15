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

#[cfg(test)]
mod tests {
    use super::*;
    use recorder_errors::{enverr, target, usage};

    #[test]
    fn map_recorder_error_sets_python_attributes() {
        Python::with_gil(|py| {
            let err = usage!(ErrorCode::UnsupportedFormat, "invalid trace format")
                .with_context("format", "yaml")
                .with_source(std::io::Error::new(std::io::ErrorKind::Other, "boom"));
            let pyerr = map_recorder_error(err);
            let ty = pyerr.get_type(py);
            assert!(ty.is(py.get_type::<PyUsageError>()));
            let value = pyerr.value(py);
            assert_eq!(
                value
                    .getattr("code")
                    .expect("error code attribute")
                    .extract::<String>()
                    .expect("code string"),
                "ERR_UNSUPPORTED_FORMAT"
            );
            assert_eq!(
                value
                    .getattr("kind")
                    .expect("error kind attribute")
                    .extract::<String>()
                    .expect("kind string"),
                "Usage"
            );
            let context_obj = value.getattr("context").expect("context attribute");
            let ctx = context_obj
                .downcast::<PyDict>()
                .expect("context attribute downcast");
            let format_value = ctx
                .get_item("format")
                .expect("context lookup failed")
                .expect("context map missing 'format'");
            assert_eq!(
                format_value
                    .extract::<String>()
                    .expect("format value extraction"),
                "yaml"
            );
        });
    }

    #[test]
    fn dispatch_converts_recorder_error_to_pyerr() {
        Python::with_gil(|py| {
            let result: PyResult<()> =
                dispatch("dispatch_env", || Err(enverr!(ErrorCode::Io, "disk full")));
            let err = result.expect_err("expected PyErr");
            let ty = err.get_type(py);
            assert!(ty.is(py.get_type::<PyEnvironmentError>()));
        });
    }

    #[test]
    fn dispatch_converts_panic_into_internal_error() {
        Python::with_gil(|py| {
            let result: PyResult<()> = dispatch("dispatch_panic", || panic!("boom"));
            let err = result.expect_err("expected panic to map into PyErr");
            let ty = err.get_type(py);
            assert!(ty.is(py.get_type::<PyInternalError>()));
            assert!(err.to_string().contains("panic in dispatch_panic"));
        });
    }

    #[test]
    fn wrap_pyfunction_passes_through_success() {
        let result = wrap_pyfunction("wrap_ok", || Ok::<_, PyErr>(42));
        assert_eq!(result.expect("expected success"), 42);
    }

    #[test]
    fn wrap_pyfunction_converts_errors_and_panics() {
        Python::with_gil(|py| {
            let err = wrap_pyfunction("wrap_error", || -> PyResult<()> {
                Err(map_recorder_error(target!(
                    ErrorCode::TraceIncomplete,
                    "target failure"
                )))
            })
            .expect_err("expected error");
            assert!(err.get_type(py).is(py.get_type::<PyTargetError>()));

            let panic_err = wrap_pyfunction("wrap_panic", || -> PyResult<()> {
                panic!("boom");
            })
            .expect_err("expected panic");
            assert!(panic_err.get_type(py).is(py.get_type::<PyInternalError>()));
            assert!(panic_err.to_string().contains("panic in wrap_panic"));
        });
    }
}
