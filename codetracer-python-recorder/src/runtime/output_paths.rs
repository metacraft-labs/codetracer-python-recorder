use std::error::Error;
use std::path::{Path, PathBuf};

use runtime_tracing::{Line, NonStreamingTraceWriter, TraceEventsFileFormat, TraceWriter};

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
    ) -> Result<(), Box<dyn Error>> {
        TraceWriter::begin_writing_trace_metadata(writer, self.metadata())?;
        TraceWriter::begin_writing_trace_paths(writer, self.paths())?;
        TraceWriter::begin_writing_trace_events(writer, self.events())?;
        TraceWriter::start(writer, start_path, Line(start_line as i64));
        Ok(())
    }
}
