//! PyO3 entry points for starting and managing trace sessions.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::logging::init_rust_logging_with_default;
use crate::monitoring::{flush_installed_tracer, install_tracer, uninstall_tracer};
use crate::runtime::{RuntimeTracer, TraceOutputPaths};
use runtime_tracing::TraceEventsFileFormat;

/// Global flag tracking whether tracing is active.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Map human-friendly strings to `TraceEventsFileFormat` variants.
fn parse_trace_format(format: &str) -> PyResult<TraceEventsFileFormat> {
    match format.to_ascii_lowercase().as_str() {
        "json" => Ok(TraceEventsFileFormat::Json),
        // Accept historical aliases that may still be referenced by callers.
        "binary" | "binaryv0" | "binary_v0" | "b0" => Ok(TraceEventsFileFormat::BinaryV0),
        other => Err(PyRuntimeError::new_err(format!(
            "unsupported trace format '{other}'. Expected one of: json, binary"
        ))),
    }
}

/// Start tracing using sys.monitoring and runtime_tracing writer.
#[pyfunction]
pub fn start_tracing(path: &str, format: &str, activation_path: Option<&str>) -> PyResult<()> {
    // Ensure logging is ready before any tracer logs might be emitted.
    // Default only our crate to debug to avoid excessive verbosity from deps.
    init_rust_logging_with_default("codetracer_python_recorder=debug");
    if ACTIVE.load(Ordering::SeqCst) {
        return Err(PyRuntimeError::new_err("tracing already active"));
    }

    // Interpret `path` as a directory where trace files will be written.
    let out_dir = Path::new(path);
    if out_dir.exists() && !out_dir.is_dir() {
        return Err(PyRuntimeError::new_err(
            "trace path exists and is not a directory",
        ));
    }
    if !out_dir.exists() {
        // Best-effort create the directory tree
        fs::create_dir_all(&out_dir).map_err(|e| {
            PyRuntimeError::new_err(format!("failed to create trace directory: {}", e))
        })?;
    }

    let fmt = parse_trace_format(format)?;

    let outputs = TraceOutputPaths::new(out_dir, fmt);

    // Activation path: when set, tracing starts only after entering it.
    let activation_path = activation_path.map(|s| Path::new(s));

    Python::with_gil(|py| {
        // Program and args: keep minimal; Python-side API stores full session info if needed
        let sys = py.import("sys")?;
        let argv = sys.getattr("argv")?;
        let program = match argv.get_item(0) {
            Ok(obj) => obj.extract::<String>()?,
            Err(_) => String::from("<unknown>"),
        };
        let args = match argv.len() {
            Ok(len) if len > 1 => {
                let mut items = Vec::with_capacity(len.saturating_sub(1));
                for idx in 1..len {
                    let value: String = argv.get_item(idx)?.extract()?;
                    items.push(value);
                }
                items
            }
            _ => Vec::new(),
        };

        let mut tracer = RuntimeTracer::new(&program, &args, fmt, activation_path);
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
