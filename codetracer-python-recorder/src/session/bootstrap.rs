//! Helpers for preparing a tracing session before installing the runtime tracer.

use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pyo3::prelude::*;
use recorder_errors::{enverr, usage, ErrorCode};
use runtime_tracing::TraceEventsFileFormat;

use crate::errors::Result;
use crate::trace_filter::config::TraceFilterConfig;
use crate::trace_filter::engine::TraceFilterEngine;

/// Basic metadata about the currently running Python program.
#[derive(Debug, Clone)]
pub struct ProgramMetadata {
    pub program: String,
    pub args: Vec<String>,
}

/// Collected data required to start a tracing session.
#[derive(Clone)]
pub struct TraceSessionBootstrap {
    trace_directory: PathBuf,
    format: TraceEventsFileFormat,
    activation_path: Option<PathBuf>,
    metadata: ProgramMetadata,
    trace_filter: Option<Arc<TraceFilterEngine>>,
}

const TRACE_FILTER_DIR: &str = ".codetracer";
const TRACE_FILTER_FILE: &str = "trace-filter.toml";

impl fmt::Debug for TraceSessionBootstrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TraceSessionBootstrap")
            .field("trace_directory", &self.trace_directory)
            .field("format", &self.format)
            .field("activation_path", &self.activation_path)
            .field("metadata", &self.metadata)
            .field("trace_filter", &self.trace_filter.is_some())
            .finish()
    }
}

impl TraceSessionBootstrap {
    /// Prepare a tracing session by validating the output directory, resolving the
    /// requested format and capturing program metadata.
    pub fn prepare(
        py: Python<'_>,
        trace_directory: &Path,
        format: &str,
        activation_path: Option<&Path>,
        explicit_trace_filters: Option<&[PathBuf]>,
    ) -> Result<Self> {
        ensure_trace_directory(trace_directory)?;
        let format = resolve_trace_format(format)?;
        let metadata = collect_program_metadata(py).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to collect program metadata")
                .with_context("details", err.to_string())
        })?;
        let trace_filter = load_trace_filter(explicit_trace_filters, &metadata.program)?;
        Ok(Self {
            trace_directory: trace_directory.to_path_buf(),
            format,
            activation_path: activation_path.map(|p| p.to_path_buf()),
            metadata,
            trace_filter,
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

    pub fn trace_filter(&self) -> Option<Arc<TraceFilterEngine>> {
        self.trace_filter.as_ref().map(Arc::clone)
    }
}

/// Ensure the requested trace directory exists and is writable.
pub fn ensure_trace_directory(path: &Path) -> Result<()> {
    if path.exists() {
        if !path.is_dir() {
            return Err(usage!(
                ErrorCode::TraceDirectoryConflict,
                "trace path exists and is not a directory"
            )
            .with_context("path", path.display().to_string()));
        }
        return Ok(());
    }

    fs::create_dir_all(path).map_err(|e| {
        enverr!(
            ErrorCode::TraceDirectoryCreateFailed,
            "failed to create trace directory"
        )
        .with_context("path", path.display().to_string())
        .with_context("io", e.to_string())
    })
}

/// Convert a user-provided format string into the runtime representation.
pub fn resolve_trace_format(value: &str) -> Result<TraceEventsFileFormat> {
    match value.to_ascii_lowercase().as_str() {
        "json" => Ok(TraceEventsFileFormat::Json),
        // Accept historical aliases for the binary format.
        "binary" | "binaryv0" | "binary_v0" | "b0" => Ok(TraceEventsFileFormat::BinaryV0),
        other => Err(usage!(
            ErrorCode::UnsupportedFormat,
            "unsupported trace format '{}'. Expected one of: json, binary",
            other
        )),
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

fn load_trace_filter(
    explicit: Option<&[PathBuf]>,
    program: &str,
) -> Result<Option<Arc<TraceFilterEngine>>> {
    let mut chain: Vec<PathBuf> = Vec::new();

    if let Some(default) = discover_default_trace_filter(program)? {
        chain.push(default);
    }

    if let Some(paths) = explicit {
        chain.extend(paths.iter().cloned());
    }

    if chain.is_empty() {
        return Ok(None);
    }

    let config = TraceFilterConfig::from_paths(&chain)?;
    Ok(Some(Arc::new(TraceFilterEngine::new(config))))
}

fn discover_default_trace_filter(program: &str) -> Result<Option<PathBuf>> {
    let start_dir = resolve_program_directory(program)?;
    let mut current: Option<&Path> = Some(start_dir.as_path());
    while let Some(dir) = current {
        let candidate = dir.join(TRACE_FILTER_DIR).join(TRACE_FILTER_FILE);
        if matches!(fs::metadata(&candidate), Ok(metadata) if metadata.is_file()) {
            return Ok(Some(candidate));
        }
        current = dir.parent();
    }
    Ok(None)
}

fn resolve_program_directory(program: &str) -> Result<PathBuf> {
    let trimmed = program.trim();
    if trimmed.is_empty() || trimmed == "<unknown>" {
        return current_directory();
    }

    let path = Path::new(trimmed);
    if path.is_absolute() {
        if path.is_dir() {
            return Ok(path.to_path_buf());
        }
        if let Some(parent) = path.parent() {
            return Ok(parent.to_path_buf());
        }
        return current_directory();
    }

    let cwd = current_directory()?;
    let joined = cwd.join(path);
    if joined.is_dir() {
        return Ok(joined);
    }
    if let Some(parent) = joined.parent() {
        return Ok(parent.to_path_buf());
    }
    Ok(cwd)
}

fn current_directory() -> Result<PathBuf> {
    env::current_dir().map_err(|err| {
        enverr!(ErrorCode::Io, "failed to resolve current directory")
            .with_context("io", err.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyList;
    use recorder_errors::ErrorCode;
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
        let err = ensure_trace_directory(&file_path).expect_err("should reject file path");
        assert_eq!(err.code, ErrorCode::TraceDirectoryConflict);
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
        let err = resolve_trace_format("yaml").expect_err("should reject yaml");
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
        assert!(err.message().contains("unsupported trace format"));
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
                None,
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

    #[test]
    fn prepare_bootstrap_ignores_missing_trace_filter() {
        Python::with_gil(|py| {
            let tmp = tempdir().expect("tempdir");
            let trace_dir = tmp.path().join("out");
            let script_path = tmp.path().join("app.py");
            std::fs::write(&script_path, "print('hello')\n").expect("write script");

            let sys = py.import("sys").expect("import sys");
            let original = sys.getattr("argv").expect("argv").unbind();
            let argv = PyList::new(py, [script_path.to_str().expect("utf8 path")]).expect("argv");
            sys.setattr("argv", argv).expect("set argv");

            let result =
                TraceSessionBootstrap::prepare(py, trace_dir.as_path(), "json", None, None);
            sys.setattr("argv", original.bind(py))
                .expect("restore argv");

            let bootstrap = result.expect("bootstrap");
            assert!(bootstrap.trace_filter().is_none());
        });
    }

    #[test]
    fn prepare_bootstrap_loads_default_trace_filter() {
        Python::with_gil(|py| {
            let project = tempdir().expect("project");
            let project_root = project.path();
            let trace_dir = project_root.join("out");

            let app_dir = project_root.join("src");
            std::fs::create_dir_all(&app_dir).expect("create src dir");
            let script_path = app_dir.join("main.py");
            std::fs::write(&script_path, "print('run')\n").expect("write script");

            let filters_dir = project_root.join(TRACE_FILTER_DIR);
            std::fs::create_dir(&filters_dir).expect("create filter dir");
            let filter_path = filters_dir.join(TRACE_FILTER_FILE);
            std::fs::write(
                &filter_path,
                r#"
                [meta]
                name = "default"
                version = 1

                [scope]
                default_exec = "trace"
                default_value_action = "allow"

                [[scope.rules]]
                selector = "pkg:src"
                exec = "trace"
                value_default = "allow"
                "#,
            )
            .expect("write filter");

            let sys = py.import("sys").expect("import sys");
            let original = sys.getattr("argv").expect("argv").unbind();
            let argv = PyList::new(py, [script_path.to_str().expect("utf8 path")]).expect("argv");
            sys.setattr("argv", argv).expect("set argv");

            let result =
                TraceSessionBootstrap::prepare(py, trace_dir.as_path(), "json", None, None);
            sys.setattr("argv", original.bind(py))
                .expect("restore argv");

            let bootstrap = result.expect("bootstrap");
            let engine = bootstrap.trace_filter().expect("filter engine");
            let summary = engine.summary();
            assert_eq!(summary.entries.len(), 1);
            assert_eq!(summary.entries[0].path, filter_path);
        });
    }

    #[test]
    fn prepare_bootstrap_merges_explicit_trace_filters() {
        Python::with_gil(|py| {
            let project = tempdir().expect("project");
            let project_root = project.path();
            let trace_dir = project_root.join("out");

            let app_dir = project_root.join("src");
            std::fs::create_dir_all(&app_dir).expect("create src dir");
            let script_path = app_dir.join("main.py");
            std::fs::write(&script_path, "print('run')\n").expect("write script");

            let filters_dir = project_root.join(TRACE_FILTER_DIR);
            std::fs::create_dir(&filters_dir).expect("create filter dir");
            let default_filter_path = filters_dir.join(TRACE_FILTER_FILE);
            std::fs::write(
                &default_filter_path,
                r#"
                [meta]
                name = "default"
                version = 1

                [scope]
                default_exec = "trace"
                default_value_action = "allow"

                [[scope.rules]]
                selector = "pkg:src"
                exec = "trace"
                value_default = "allow"
                "#,
            )
            .expect("write default filter");

            let override_filter_path = project_root.join("override-filter.toml");
            std::fs::write(
                &override_filter_path,
                r#"
                [meta]
                name = "override"
                version = 1

                [scope]
                default_exec = "trace"
                default_value_action = "allow"

                [[scope.rules]]
                selector = "pkg:src.special"
                exec = "skip"
                value_default = "deny"
                "#,
            )
            .expect("write override filter");

            let sys = py.import("sys").expect("import sys");
            let original = sys.getattr("argv").expect("argv").unbind();
            let argv = PyList::new(py, [script_path.to_str().expect("utf8 path")]).expect("argv");
            sys.setattr("argv", argv).expect("set argv");

            let explicit = vec![override_filter_path.clone()];
            let result = TraceSessionBootstrap::prepare(
                py,
                trace_dir.as_path(),
                "json",
                None,
                Some(explicit.as_slice()),
            );
            sys.setattr("argv", original.bind(py))
                .expect("restore argv");

            let bootstrap = result.expect("bootstrap");
            let engine = bootstrap.trace_filter().expect("filter engine");
            let summary = engine.summary();
            assert_eq!(summary.entries.len(), 2);
            assert_eq!(summary.entries[0].path, default_filter_path);
            assert_eq!(summary.entries[1].path, override_filter_path);
        });
    }
}
