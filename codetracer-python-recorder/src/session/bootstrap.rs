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

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyList;
    use tempfile::tempdir;

    #[test]
    fn ensure_trace_directory_creates_missing_dir() {
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join("trace-out");
        ensure_trace_directory(&target).expect("create directory");
        assert!(target.is_dir());
    }

    #[test]
    fn ensure_trace_directory_rejects_file_path() {
        let tmp = tempdir().expect("tempdir");
        let file_path = tmp.path().join("trace.bin");
        std::fs::write(&file_path, b"stub").expect("write stub file");
        assert!(ensure_trace_directory(&file_path).is_err());
    }

    #[test]
    fn resolve_trace_format_accepts_supported_aliases() {
        assert!(matches!(
            resolve_trace_format("json").expect("json format"),
            TraceEventsFileFormat::Json
        ));
        assert!(matches!(
            resolve_trace_format("BiNaRy").expect("binary alias"),
            TraceEventsFileFormat::BinaryV0
        ));
    }

    #[test]
    fn resolve_trace_format_rejects_unknown_values() {
        Python::with_gil(|py| {
            let err = resolve_trace_format("yaml").expect_err("should reject yaml");
            assert_eq!(err.get_type(py).name().expect("type name"), "RuntimeError");
            let message = err.value(py).to_string();
            assert!(message.contains("unsupported trace format"));
        });
    }

    #[test]
    fn collect_program_metadata_reads_sys_argv() {
        Python::with_gil(|py| {
            let sys = py.import("sys").expect("import sys");
            let original = sys.getattr("argv").expect("argv").unbind();
            let argv = PyList::new(py, ["/tmp/prog.py", "--flag", "value"]).expect("argv");
            sys.setattr("argv", argv).expect("set argv");

            let result = collect_program_metadata(py);
            sys.setattr("argv", original.bind(py))
                .expect("restore argv");

            let metadata = result.expect("metadata");
            assert_eq!(metadata.program, "/tmp/prog.py");
            assert_eq!(
                metadata.args,
                vec!["--flag".to_string(), "value".to_string()]
            );
        });
    }

    #[test]
    fn collect_program_metadata_defaults_unknown_program() {
        Python::with_gil(|py| {
            let sys = py.import("sys").expect("import sys");
            let original = sys.getattr("argv").expect("argv").unbind();
            let empty = PyList::empty(py);
            sys.setattr("argv", empty).expect("set empty argv");

            let result = collect_program_metadata(py);
            sys.setattr("argv", original.bind(py))
                .expect("restore argv");

            let metadata = result.expect("metadata");
            assert_eq!(metadata.program, "<unknown>");
            assert!(metadata.args.is_empty());
        });
    }

    #[test]
    fn prepare_bootstrap_populates_fields_and_creates_directory() {
        Python::with_gil(|py| {
            let tmp = tempdir().expect("tempdir");
            let trace_dir = tmp.path().join("out");
            let activation = tmp.path().join("entry.py");
            std::fs::write(&activation, "print('hi')\n").expect("write activation file");

            let sys = py.import("sys").expect("import sys");
            let original = sys.getattr("argv").expect("argv").unbind();
            let program_str = activation.to_str().expect("utf8 path");
            let argv = PyList::new(py, [program_str, "--verbose"]).expect("argv");
            sys.setattr("argv", argv).expect("set argv");

            let result = TraceSessionBootstrap::prepare(
                py,
                trace_dir.as_path(),
                "json",
                Some(activation.as_path()),
            );
            sys.setattr("argv", original.bind(py))
                .expect("restore argv");

            let bootstrap = result.expect("bootstrap");
            assert!(trace_dir.is_dir());
            assert_eq!(bootstrap.trace_directory(), trace_dir.as_path());
            assert!(matches!(bootstrap.format(), TraceEventsFileFormat::Json));
            assert_eq!(bootstrap.activation_path(), Some(activation.as_path()));
            assert_eq!(bootstrap.program(), program_str);
            let expected_args: Vec<String> = vec!["--verbose".to_string()];
            assert_eq!(bootstrap.args(), expected_args.as_slice());
        });
    }
}
