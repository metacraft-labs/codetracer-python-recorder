//! File-system helpers for trace output management.

use std::path::{Path, PathBuf};

use recorder_errors::{enverr, ErrorCode};
use codetracer_trace_types::Line;
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::TraceEventsFileFormat;

use crate::errors::Result;

/// File layout for a trace session. Encapsulates the events file
/// (canonical `.ct` CTFS container in CTFS mode) that needs to be
/// initialised alongside the runtime tracer.  The legacy
/// `trace_metadata.json` and `trace_paths.json` operational sidecars
/// were retired with the v3 CTFS rollout (follow-up #254 phase 2);
/// program / paths metadata now lives in `meta.dat` inside the
/// container.
#[derive(Debug, Clone)]
pub struct TraceOutputPaths {
    events: PathBuf,
}

impl TraceOutputPaths {
    /// Build output paths for a given directory. The directory is expected to
    /// exist before initialisation; callers should ensure it is created.
    pub fn new(root: &Path, format: TraceEventsFileFormat) -> Self {
        let events_name = match format {
            TraceEventsFileFormat::Json => "trace.json",
            TraceEventsFileFormat::Ctfs => "trace.ct",
            _ => "trace.bin",
        };
        Self {
            events: root.join(events_name),
        }
    }

    pub fn events(&self) -> &Path {
        &self.events
    }

    /// Wire the trace writer to the configured output files and record the
    /// initial start location.
    pub fn configure_writer(
        &self,
        writer: &mut dyn TraceWriter,
        start_path: &Path,
        start_line: u32,
    ) -> Result<()> {
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
    use codetracer_trace_types::{Line, TraceLowLevelEvent};
    use codetracer_trace_writer_nim::non_streaming_trace_writer::NonStreamingTraceWriter;
    use tempfile::tempdir;

    #[test]
    fn json_paths_use_json_filenames() {
        let tmp = tempdir().expect("tempdir");
        let paths = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::Json);
        assert_eq!(paths.events(), tmp.path().join("trace.json").as_path());
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
