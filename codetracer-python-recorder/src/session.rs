//! PyO3 entry points for starting and managing trace sessions.

mod bootstrap;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::prelude::*;
use recorder_errors::{usage, ErrorCode};

use crate::ffi;
use crate::logging::init_rust_logging_with_default;
use crate::monitoring::{
    flush_installed_tracer,
    install_tracer,
    uninstall_tracer,
    update_exit_status,
};
use crate::policy::policy_snapshot;
use crate::runtime::{RuntimeTracer, TraceOutputPaths};
use bootstrap::TraceSessionBootstrap;

/// Global flag tracking whether tracing is active.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Start tracing using sys.monitoring and runtime_tracing writer.
#[pyfunction(signature = (path, format, activation_path=None, trace_filter=None))]
pub fn start_tracing(
    path: &str,
    format: &str,
    activation_path: Option<&str>,
    trace_filter: Option<Vec<String>>,
) -> PyResult<()> {
    ffi::wrap_pyfunction("start_tracing", || {
        // Ensure logging is ready before any tracer logs might be emitted.
        // Default our crate to warnings-only so tests stay quiet unless explicitly enabled.
        init_rust_logging_with_default("codetracer_python_recorder=warn");
        if ACTIVE.load(Ordering::SeqCst) {
            return Err(ffi::map_recorder_error(usage!(
                ErrorCode::AlreadyTracing,
                "tracing already active"
            )));
        }

        let activation_path = activation_path.map(PathBuf::from);
        let filter_paths: Option<Vec<PathBuf>> =
            trace_filter.map(|items| items.into_iter().map(PathBuf::from).collect());

        Python::with_gil(|py| {
            let bootstrap = TraceSessionBootstrap::prepare(
                py,
                Path::new(path),
                format,
                activation_path.as_deref(),
                filter_paths.as_ref().map(|paths| paths.as_slice()),
            )
            .map_err(ffi::map_recorder_error)?;

            let outputs = TraceOutputPaths::new(bootstrap.trace_directory(), bootstrap.format());
            let policy = policy_snapshot();

            let mut tracer = RuntimeTracer::new(
                bootstrap.program(),
                bootstrap.args(),
                bootstrap.format(),
                bootstrap.activation_path(),
                bootstrap.trace_filter(),
            );
            tracer.begin(&outputs, 1)?;
            tracer.install_io_capture(py, &policy)?;

            // Install callbacks
            install_tracer(py, Box::new(tracer))?;
            ACTIVE.store(true, Ordering::SeqCst);
            Ok(())
        })
    })
}

/// Stop tracing by resetting the global flag.
#[pyfunction(signature = (exit_code=None))]
pub fn stop_tracing(exit_code: Option<i32>) -> PyResult<()> {
    ffi::wrap_pyfunction("stop_tracing", || {
        Python::with_gil(|py| {
            update_exit_status(py, exit_code)?;
            // Uninstall triggers finish() on tracer implementation.
            uninstall_tracer(py)?;
            ACTIVE.store(false, Ordering::SeqCst);
            Ok(())
        })
    })
}

/// Query whether tracing is currently active.
#[pyfunction]
pub fn is_tracing() -> PyResult<bool> {
    ffi::wrap_pyfunction("is_tracing", || Ok(ACTIVE.load(Ordering::SeqCst)))
}

/// Flush buffered trace data (best-effort, non-streaming formats only).
#[pyfunction]
pub fn flush_tracing() -> PyResult<()> {
    ffi::wrap_pyfunction("flush_tracing", || {
        Python::with_gil(|py| flush_installed_tracer(py))
    })
}
