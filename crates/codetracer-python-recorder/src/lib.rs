use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

/// Global flag tracking whether tracing is active.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Start tracing. Placeholder implementation that simply flips the
/// global active flag and ignores all parameters.
#[pyfunction]
fn start_tracing(
    _path: &str,
    _format: &str,
    _capture_values: bool,
    _source_roots: Option<Vec<String>>,
) -> PyResult<()> {
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return Err(PyRuntimeError::new_err("tracing already active"));
    }
    Ok(())
}

/// Stop tracing by resetting the global flag.
#[pyfunction]
fn stop_tracing() -> PyResult<()> {
    ACTIVE.store(false, Ordering::SeqCst);
    Ok(())
}

/// Query whether tracing is currently active.
#[pyfunction]
fn is_tracing() -> PyResult<bool> {
    Ok(ACTIVE.load(Ordering::SeqCst))
}

/// Flush buffered trace data. No-op placeholder for now.
#[pyfunction]
fn flush_tracing() -> PyResult<()> {
    Ok(())
}

/// Trivial function kept for smoke tests verifying the module builds.
#[pyfunction]
fn hello() -> PyResult<String> {
    Ok("Hello from codetracer-python-recorder (Rust)".to_string())
}

/// Python module definition.
#[pymodule]
fn codetracer_python_recorder(_py: Python<'_>, m: Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(start_tracing, &m)?)?;
    m.add_function(wrap_pyfunction!(stop_tracing, &m)?)?;
    m.add_function(wrap_pyfunction!(is_tracing, &m)?)?;
    m.add_function(wrap_pyfunction!(flush_tracing, &m)?)?;
    m.add_function(wrap_pyfunction!(hello, &m)?)?;
    Ok(())
}
