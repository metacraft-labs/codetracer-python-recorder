//! File-system helpers for trace output management.

use std::path::{Path, PathBuf};

use recorder_errors::{enverr, ErrorCode};
use runtime_tracing::{Line, NonStreamingTraceWriter, TraceEventsFileFormat, TraceWriter};

use crate::errors::Result;

/// File layout for a trace session. Encapsulates the metadata, event, and paths
/// files that need to be initialised alongside the runtime tracer.
#[derive(Debug, Clone)]
pub struct TraceOutputPaths {
    events: PathBuf,
    metadata: PathBuf,
    paths: PathBuf,
}

impl TraceOutputPaths {
    /// Build output paths for a given directory. The directory is expected to
    /// exist before initialisation; callers should ensure it is created.
    pub fn new(root: &Path, format: TraceEventsFileFormat) -> Self {
        let (events_name, metadata_name, paths_name) = match format {
            TraceEventsFileFormat::Json => {
                ("trace.json", "trace_metadata.json", "trace_paths.json")
            }
            _ => ("trace.bin", "trace_metadata.json", "trace_paths.json"),
        };
        Self {
            events: root.join(events_name),
            metadata: root.join(metadata_name),
            paths: root.join(paths_name),
        }
    }

    pub fn events(&self) -> &Path {
        &self.events
    }

    pub fn metadata(&self) -> &Path {
        &self.metadata
    }

    pub fn paths(&self) -> &Path {
        &self.paths
    }

    /// Wire the trace writer to the configured output files and record the
    /// initial start location.
    pub fn configure_writer(
        &self,
        writer: &mut NonStreamingTraceWriter,
        start_path: &Path,
        start_line: u32,
    ) -> Result<()> {
        TraceWriter::begin_writing_trace_metadata(writer, self.metadata()).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to begin trace metadata")
                .with_context("path", self.metadata().display().to_string())
                .with_context("source", err.to_string())
        })?;
        TraceWriter::begin_writing_trace_paths(writer, self.paths()).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to begin trace paths")
                .with_context("path", self.paths().display().to_string())
                .with_context("source", err.to_string())
        })?;
        TraceWriter::begin_writing_trace_events(writer, self.events()).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to begin trace events")
                .with_context("path", self.events().display().to_string())
                .with_context("source", err.to_string())
        })?;
        TraceWriter::start(writer, start_path, Line(start_line as i64));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_tracing::{Line, TraceLowLevelEvent};
    use tempfile::tempdir;

    #[test]
    fn json_paths_use_json_filenames() {
        let tmp = tempdir().expect("tempdir");
        let paths = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::Json);
        assert_eq!(paths.events(), tmp.path().join("trace.json").as_path());
        assert_eq!(
            paths.metadata(),
            tmp.path().join("trace_metadata.json").as_path()
        );
        assert_eq!(paths.paths(), tmp.path().join("trace_paths.json").as_path());
    }

    #[test]
    fn binary_paths_use_bin_extension() {
        let tmp = tempdir().expect("tempdir");
        let paths = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::BinaryV0);
        assert_eq!(paths.events(), tmp.path().join("trace.bin").as_path());
    }

    #[test]
    fn configure_writer_initialises_writer_state() {
        let tmp = tempdir().expect("tempdir");
        let start_path = tmp.path().join("program.py");
        std::fs::write(&start_path, "print('hi')\n").expect("write script");

        let paths = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::Json);
        let mut writer = NonStreamingTraceWriter::new("program.py", &[]);

        paths
            .configure_writer(&mut writer, &start_path, 123)
            .expect("configure writer");

        let recorded_path = writer.events.iter().find_map(|event| match event {
            TraceLowLevelEvent::Path(p) => Some(p.clone()),
            _ => None,
        });
        assert_eq!(recorded_path.as_deref(), Some(start_path.as_path()));

        let function_record = writer.events.iter().find_map(|event| match event {
            TraceLowLevelEvent::Function(record) => Some(record.clone()),
            _ => None,
        });
        let record = function_record.expect("function record");
        assert_eq!(record.line, Line(123));

        let has_call = writer
            .events
            .iter()
            .any(|event| matches!(event, TraceLowLevelEvent::Call(_)));
        assert!(has_call, "expected toplevel call event");
    }
}
