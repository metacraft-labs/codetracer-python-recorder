//! Helpers for preparing a tracing session before installing the runtime tracer.

use std::fs;
use std::path::{Path, PathBuf};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use runtime_tracing::TraceEventsFileFormat;

/// Basic metadata about the currently running Python program.
#[derive(Debug, Clone)]
pub struct ProgramMetadata {
    pub program: String,
    pub args: Vec<String>,
}

/// Collected data required to start a tracing session.
#[derive(Debug, Clone)]
pub struct TraceSessionBootstrap {
    trace_directory: PathBuf,
    format: TraceEventsFileFormat,
    activation_path: Option<PathBuf>,
    metadata: ProgramMetadata,
}

impl TraceSessionBootstrap {
    /// Prepare a tracing session by validating the output directory, resolving the
    /// requested format and capturing program metadata.
    pub fn prepare(
        py: Python<'_>,
        trace_directory: &Path,
        format: &str,
        activation_path: Option<&Path>,
    ) -> PyResult<Self> {
        ensure_trace_directory(trace_directory)?;
        let format = resolve_trace_format(format)?;
        let metadata = collect_program_metadata(py)?;
        Ok(Self {
            trace_directory: trace_directory.to_path_buf(),
            format,
            activation_path: activation_path.map(|p| p.to_path_buf()),
            metadata,
        })
    }

    pub fn trace_directory(&self) -> &Path {
        &self.trace_directory
    }

    pub fn format(&self) -> TraceEventsFileFormat {
        self.format
    }

    pub fn activation_path(&self) -> Option<&Path> {
        self.activation_path.as_deref()
    }

    pub fn program(&self) -> &str {
        &self.metadata.program
    }

    pub fn args(&self) -> &[String] {
        &self.metadata.args
    }
}

/// Ensure the requested trace directory exists and is writable.
pub fn ensure_trace_directory(path: &Path) -> PyResult<()> {
    if path.exists() {
        if !path.is_dir() {
            return Err(PyRuntimeError::new_err(
                "trace path exists and is not a directory",
            ));
        }
        return Ok(());
    }

    fs::create_dir_all(path).map_err(|e| {
        PyRuntimeError::new_err(format!(
            "failed to create trace directory '{}': {e}",
            path.display()
        ))
    })
}

/// Convert a user-provided format string into the runtime representation.
pub fn resolve_trace_format(value: &str) -> PyResult<TraceEventsFileFormat> {
    match value.to_ascii_lowercase().as_str() {
        "json" => Ok(TraceEventsFileFormat::Json),
        // Accept historical aliases for the binary format.
        "binary" | "binaryv0" | "binary_v0" | "b0" => Ok(TraceEventsFileFormat::BinaryV0),
        other => Err(PyRuntimeError::new_err(format!(
            "unsupported trace format '{other}'. Expected one of: json, binary"
        ))),
    }
}

/// Capture program name and arguments from `sys.argv` for metadata records.
pub fn collect_program_metadata(py: Python<'_>) -> PyResult<ProgramMetadata> {
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

    Ok(ProgramMetadata { program, args })
}
