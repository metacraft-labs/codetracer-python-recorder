//! Helpers for preparing a tracing session before installing the runtime tracer.

mod filesystem;
mod filters;
mod metadata;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pyo3::prelude::*;
use runtime_tracing::TraceEventsFileFormat;

use crate::errors::Result;
use crate::trace_filter::engine::TraceFilterEngine;
use filesystem::{ensure_trace_directory, resolve_trace_format};
use filters::load_trace_filter;
use metadata::collect_program_metadata;

/// Basic metadata about the currently running Python program.
pub use metadata::ProgramMetadata;

/// Collected data required to start a tracing session.
#[derive(Clone)]
pub struct TraceSessionBootstrap {
    trace_directory: PathBuf,
    format: TraceEventsFileFormat,
    activation_path: Option<PathBuf>,
    metadata: ProgramMetadata,
    trace_filter: Option<Arc<TraceFilterEngine>>,
}

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
        let metadata = collect_program_metadata(py)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use metadata::tests::{with_sys_argv, ProgramArgs};
    use recorder_errors::ErrorCode;
    use std::path::PathBuf;
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
            let metadata = with_sys_argv(
                py,
                ProgramArgs::new(["/tmp/prog.py", "--flag", "value"]),
                || collect_program_metadata(py),
            )
            .expect("metadata");
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
            let metadata = with_sys_argv(py, ProgramArgs::empty(), || collect_program_metadata(py))
                .expect("metadata");
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

            let program_str = activation.to_str().expect("utf8 path");
            let result = with_sys_argv(py, ProgramArgs::new([program_str, "--verbose"]), || {
                TraceSessionBootstrap::prepare(
                    py,
                    trace_dir.as_path(),
                    "json",
                    Some(activation.as_path()),
                    None,
                )
            });

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
    fn prepare_bootstrap_applies_builtin_trace_filter() {
        Python::with_gil(|py| {
            let tmp = tempdir().expect("tempdir");
            let trace_dir = tmp.path().join("out");
            let script_path = tmp.path().join("app.py");
            std::fs::write(&script_path, "print('hello')\n").expect("write script");

            let result = with_sys_argv(
                py,
                ProgramArgs::new([script_path.to_str().expect("utf8 path")]),
                || TraceSessionBootstrap::prepare(py, trace_dir.as_path(), "json", None, None),
            );

            let bootstrap = result.expect("bootstrap");
            let engine = bootstrap.trace_filter().expect("builtin filter");
            let summary = engine.summary();
            assert_eq!(summary.entries.len(), 1);
            assert_eq!(
                summary.entries[0].path,
                PathBuf::from("<inline:builtin-default>")
            );
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

            let filter_path = filters::tests::write_default_filter(project_root);

            let result = with_sys_argv(
                py,
                ProgramArgs::new([script_path.to_str().expect("utf8 path")]),
                || TraceSessionBootstrap::prepare(py, trace_dir.as_path(), "json", None, None),
            );

            let bootstrap = result.expect("bootstrap");
            let engine = bootstrap.trace_filter().expect("filter engine");
            let summary = engine.summary();
            assert_eq!(summary.entries.len(), 2);
            assert_eq!(
                summary.entries[0].path,
                PathBuf::from("<inline:builtin-default>")
            );
            assert_eq!(summary.entries[1].path, filter_path);
        });
    }

    #[test]
    fn prepare_bootstrap_merges_explicit_trace_filters() {
        Python::with_gil(|py| {
            let project = tempdir().expect("project");
            let project_root = project.path();
            let trace_dir = project_root.join("out");

            let script_path = filters::tests::write_app(project_root);
            let (default_filter_path, override_filter_path) =
                filters::tests::write_default_and_override(project_root);

            let explicit = vec![override_filter_path.clone()];
            let result = with_sys_argv(
                py,
                ProgramArgs::new([script_path.to_str().expect("utf8 path")]),
                || {
                    TraceSessionBootstrap::prepare(
                        py,
                        trace_dir.as_path(),
                        "json",
                        None,
                        Some(explicit.as_slice()),
                    )
                },
            );

            let bootstrap = result.expect("bootstrap");
            let engine = bootstrap.trace_filter().expect("filter engine");
            let summary = engine.summary();
            assert_eq!(summary.entries.len(), 3);
            assert_eq!(
                summary.entries[0].path,
                PathBuf::from("<inline:builtin-default>")
            );
            assert_eq!(summary.entries[1].path, default_filter_path);
            assert_eq!(summary.entries[2].path, override_filter_path);
        });
    }
}
