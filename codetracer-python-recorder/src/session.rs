//! PyO3 entry points for starting and managing trace sessions.

mod bootstrap;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::prelude::*;
use recorder_errors::{usage, ErrorCode};

use crate::ffi;
use crate::logging::init_rust_logging_with_default;
use crate::monitoring::{flush_installed_tracer, install_tracer, uninstall_tracer};
use crate::policy::policy_snapshot;
use crate::runtime::{RuntimeTracer, TraceOutputPaths};
use bootstrap::TraceSessionBootstrap;

/// Global flag tracking whether tracing is active.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Start tracing using sys.monitoring and runtime_tracing writer.
#[pyfunction]
pub fn start_tracing(path: &str, format: &str, activation_path: Option<&str>) -> PyResult<()> {
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

        Python::with_gil(|py| {
            let bootstrap = TraceSessionBootstrap::prepare(
                py,
                Path::new(path),
                format,
                activation_path.as_deref(),
            )
            .map_err(ffi::map_recorder_error)?;

            let outputs = TraceOutputPaths::new(bootstrap.trace_directory(), bootstrap.format());
            let policy = policy_snapshot();

            let mut tracer = RuntimeTracer::new(
                bootstrap.program(),
                bootstrap.args(),
                bootstrap.format(),
                bootstrap.activation_path(),
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
#[pyfunction]
pub fn stop_tracing() -> PyResult<()> {
    ffi::wrap_pyfunction("stop_tracing", || {
        Python::with_gil(|py| {
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
