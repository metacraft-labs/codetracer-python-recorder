//! PyO3 entry points for starting and managing trace sessions.

mod bootstrap;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use log::warn;
use once_cell::sync::Lazy;
use pyo3::prelude::*;
use recorder_errors::{usage, ErrorCode};

use crate::ffi;
use crate::logging::init_rust_logging_with_default;
use crate::monitoring::{flush_installed_tracer, install_tracer, uninstall_tracer};
use crate::runtime::{RuntimeTracer, TraceOutputPaths};
use bootstrap::TraceSessionBootstrap;

/// Global flag tracking whether tracing is active.
static ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct SessionLifecycle {
    extras: Vec<Py<PyAny>>,
}

impl SessionLifecycle {
    fn replace(&mut self, extras: Option<Py<PyAny>>) {
        self.extras.clear();
        if let Some(extra) = extras {
            self.extras.push(extra);
        }
    }

    fn extend(&mut self, extras: Option<Py<PyAny>>) {
        if let Some(extra) = extras {
            self.extras.push(extra);
        }
    }

    fn drain(&mut self, py: Python<'_>) {
        for handle in self.extras.drain(..) {
            if let Err(err) = invoke_stop(py, &handle) {
                warn!("failed to stop lifecycle handle");
                err.print(py);
            }
        }
    }
}

static SESSION_LIFECYCLE: Lazy<Mutex<SessionLifecycle>> =
    Lazy::new(|| Mutex::new(SessionLifecycle::default()));

fn invoke_stop(py: Python<'_>, handle: &Py<PyAny>) -> PyResult<()> {
    let bound = handle.bind(py);
    if bound.is_none() {
        return Ok(());
    }
    if bound.hasattr("stop")? {
        bound.call_method0("stop")?;
    } else if bound.hasattr("close")? {
        bound.call_method0("close")?;
    }
    Ok(())
}

/// Start tracing using sys.monitoring and runtime_tracing writer.
#[pyfunction(signature = (path, format, activation_path=None, extras=None))]
pub fn start_tracing(
    path: &str,
    format: &str,
    activation_path: Option<&str>,
    extras: Option<Bound<'_, PyAny>>,
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

        Python::with_gil(|py| {
            let mut extra_handle = extras.map(|obj| obj.unbind());
            let bootstrap = TraceSessionBootstrap::prepare(
                py,
                Path::new(path),
                format,
                activation_path.as_deref(),
            )
            .map_err(ffi::map_recorder_error)?;

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
            if let Ok(mut lifecycle) = SESSION_LIFECYCLE.lock() {
                lifecycle.replace(extra_handle.take());
            }
            Ok(())
        })
    })
}

/// Stop tracing by resetting the global flag.
#[pyfunction(signature = (extras=None))]
pub fn stop_tracing(extras: Option<Bound<'_, PyAny>>) -> PyResult<()> {
    ffi::wrap_pyfunction("stop_tracing", || {
        Python::with_gil(|py| {
            // Uninstall triggers finish() on tracer implementation.
            uninstall_tracer(py)?;
            ACTIVE.store(false, Ordering::SeqCst);
            let extra_handle = extras.map(|obj| obj.unbind());
            if let Ok(mut lifecycle) = SESSION_LIFECYCLE.lock() {
                lifecycle.extend(extra_handle);
                lifecycle.drain(py);
            }
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
