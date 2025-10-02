//! Shared helpers for translating `RecorderError` into Python exceptions.

use std::fmt::Write as _;

use pyo3::{exceptions::PyRuntimeError, PyErr};
use recorder_errors::{RecorderError, RecorderResult};

/// Convenient alias for recorder results used across the Rust modules.
pub type Result<T> = RecorderResult<T>;

/// Convert a `RecorderError` into a `PyErr` that surfaces the stable error code
/// alongside the human-readable message and context payload.
pub fn to_py_err(err: RecorderError) -> PyErr {
    let mut message = format!("[{}] {}", err.code, err.message());
    if !err.context.is_empty() {
        let mut first = true;
        message.push_str(" (");
        for (key, value) in &err.context {
            if !first {
                message.push_str(", ");
            }
            first = false;
            let _ = write!(&mut message, "{}={}", key, value);
        }
        message.push(')');
    }
    if let Some(source) = err.source_ref() {
        let _ = write!(&mut message, ": caused by {}", source);
    }
    PyRuntimeError::new_err(message)
}
