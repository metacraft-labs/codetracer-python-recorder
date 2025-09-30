//! Runtime tracing module backed by PyO3.
//!
//! Tracer implementations must return `CallbackResult` from every callback so they can
//! signal when CPython should disable further monitoring for a location by propagating
//! the `sys.monitoring.DISABLE` sentinel.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
pub mod code_object;
mod runtime_tracer;
pub mod tracer;
pub use crate::code_object::{CodeObjectRegistry, CodeObjectWrapper};
pub use crate::tracer::{
    install_tracer, uninstall_tracer, CallbackOutcome, CallbackResult, EventSet, Tracer,
};

/// Global flag tracking whether tracing is active.
static ACTIVE: AtomicBool = AtomicBool::new(false);

// Initialize Rust logging once per process. Defaults to debug for this crate
// unless overridden by RUST_LOG. This helps surface debug! output during dev.
static INIT_LOGGER: Once = Once::new();

fn init_rust_logging_with_default(default_filter: &str) {
    INIT_LOGGER.call_once(|| {
        let env = env_logger::Env::default().default_filter_or(default_filter);
        // Use a compact format with timestamps and targets to aid debugging.
        let mut builder = env_logger::Builder::from_env(env);
        builder.format_timestamp_micros().format_target(true);
        let _ = builder.try_init();
    });
}

/// Start tracing using sys.monitoring and runtime_tracing writer.
#[pyfunction]
fn start_tracing(path: &str, format: &str, activation_path: Option<&str>) -> PyResult<()> {
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

    // Map format string to enum
    let fmt = match format.to_lowercase().as_str() {
        "json" => runtime_tracing::TraceEventsFileFormat::Json,
        // Use BinaryV0 for "binary" to avoid streaming writer here.
        "binary" | "binaryv0" | "binary_v0" | "b0" => {
            runtime_tracing::TraceEventsFileFormat::BinaryV0
        }
        //TODO AI! We need to assert! that the format is among the known values.
        other => {
            eprintln!("Unknown format '{}', defaulting to binary (v0)", other);
            runtime_tracing::TraceEventsFileFormat::BinaryV0
        }
    };

    // Build output file paths inside the directory.
    let (events_path, meta_path, paths_path) = match fmt {
        runtime_tracing::TraceEventsFileFormat::Json => (
            out_dir.join("trace.json"),
            out_dir.join("trace_metadata.json"),
            out_dir.join("trace_paths.json"),
        ),
        _ => (
            out_dir.join("trace.bin"),
            out_dir.join("trace_metadata.json"),
            out_dir.join("trace_paths.json"),
        ),
    };

    // Activation path: when set, tracing starts only after entering it.
    let activation_path = activation_path.map(|s| Path::new(s));

    Python::with_gil(|py| {
        // Program and args: keep minimal; Python-side API stores full session info if needed
        let sys = py.import("sys")?;
        let argv = sys.getattr("argv")?;
        let program: String = argv.get_item(0)?.extract::<String>()?;
        //TODO: Error-handling. What to do if argv is empty? Does this ever happen?

        let mut tracer = runtime_tracer::RuntimeTracer::new(&program, &[], fmt, activation_path);

        // Start location: prefer activation path, otherwise best-effort argv[0]
        let start_path: &Path = activation_path.unwrap_or(Path::new(&program));
        log::debug!("{}", start_path.display());
        tracer.begin(&meta_path, &paths_path, &events_path, start_path, 1)?;

        // Install callbacks
        install_tracer(py, Box::new(tracer))?;
        ACTIVE.store(true, Ordering::SeqCst);
        Ok(())
    })
}

/// Stop tracing by resetting the global flag.
#[pyfunction]
fn stop_tracing() -> PyResult<()> {
    Python::with_gil(|py| {
        // Uninstall triggers finish() on tracer implementation.
        uninstall_tracer(py)?;
        ACTIVE.store(false, Ordering::SeqCst);
        Ok(())
    })
}

/// Query whether tracing is currently active.
#[pyfunction]
fn is_tracing() -> PyResult<bool> {
    Ok(ACTIVE.load(Ordering::SeqCst))
}

/// Flush buffered trace data (best-effort, non-streaming formats only).
#[pyfunction]
fn flush_tracing() -> PyResult<()> {
    Python::with_gil(|py| crate::tracer::flush_installed_tracer(py))
}

/// Python module definition.
#[pymodule]
fn codetracer_python_recorder(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Initialize logging on import so users see logs without extra setup.
    // Respect RUST_LOG if present; otherwise default to debug for this crate.
    init_rust_logging_with_default("codetracer_python_recorder=debug");
    m.add_function(wrap_pyfunction!(start_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(stop_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(is_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(flush_tracing, m)?)?;
    Ok(())
}
