//! PyO3 entry points for starting and managing trace sessions.

mod bootstrap;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::logging::init_rust_logging_with_default;
use crate::monitoring::{flush_installed_tracer, install_tracer, uninstall_tracer};
use crate::runtime::{RuntimeTracer, TraceOutputPaths};
use bootstrap::TraceSessionBootstrap;

/// Global flag tracking whether tracing is active.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Start tracing using sys.monitoring and runtime_tracing writer.
#[pyfunction]
pub fn start_tracing(path: &str, format: &str, activation_path: Option<&str>) -> PyResult<()> {
    // Ensure logging is ready before any tracer logs might be emitted.
    // Default only our crate to debug to avoid excessive verbosity from deps.
    init_rust_logging_with_default("codetracer_python_recorder=debug");
    if ACTIVE.load(Ordering::SeqCst) {
        return Err(PyRuntimeError::new_err("tracing already active"));
    }

    let activation_path = activation_path.map(PathBuf::from);

    Python::with_gil(|py| {
        let bootstrap = TraceSessionBootstrap::prepare(
            py,
            Path::new(path),
            format,
            activation_path.as_deref(),
        )?;

        let outputs = TraceOutputPaths::new(bootstrap.trace_directory(), bootstrap.format());

        let mut tracer = RuntimeTracer::new(
            bootstrap.program(),
            bootstrap.args(),
            bootstrap.format(),
            bootstrap.activation_path(),
        );
        tracer.begin(&outputs, 1)?;

        // Install callbacks
        install_tracer(py, Box::new(tracer))?;
        ACTIVE.store(true, Ordering::SeqCst);
        Ok(())
    })
}

/// Stop tracing by resetting the global flag.
#[pyfunction]
pub fn stop_tracing() -> PyResult<()> {
    Python::with_gil(|py| {
        // Uninstall triggers finish() on tracer implementation.
        uninstall_tracer(py)?;
        ACTIVE.store(false, Ordering::SeqCst);
        Ok(())
    })
}

/// Query whether tracing is currently active.
#[pyfunction]
pub fn is_tracing() -> PyResult<bool> {
    Ok(ACTIVE.load(Ordering::SeqCst))
}

/// Flush buffered trace data (best-effort, non-streaming formats only).
#[pyfunction]
pub fn flush_tracing() -> PyResult<()> {
    Python::with_gil(|py| flush_installed_tracer(py))
}
