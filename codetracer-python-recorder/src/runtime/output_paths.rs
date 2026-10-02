//! File-system helpers for trace output management.

use std::path::{Path, PathBuf};

use codetracer_trace_types::Line;
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::TraceEventsFileFormat;
use recorder_errors::{enverr, ErrorCode};

use crate::errors::Result;
use crate::runtime::tracer::path_tables::PathTables;

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
    format: TraceEventsFileFormat,
}

impl TraceOutputPaths {
    /// Build output paths for a given directory. The directory is expected to
    /// exist before initialisation; callers should ensure it is created.
    ///
    /// The CTFS writer names its container after the recorded program,
    /// `<root>/<program stem>.ct`, so `program` must be the name the writer
    /// was constructed with.
    pub fn new(root: &Path, format: TraceEventsFileFormat, program: &str) -> Self {
        let events_name = match format {
            TraceEventsFileFormat::Ctfs => ctfs_container_name(program),
            _ => "trace.bin".to_string(),
        };
        Self {
            events: root.join(events_name),
            format,
        }
    }

    pub fn events(&self) -> &Path {
        &self.events
    }

    pub fn format(&self) -> TraceEventsFileFormat {
        self.format
    }

    /// Wire the trace writer to the configured output files and record the
    /// initial start location.
    ///
    /// P1.1 — when the writer is the canonical multi-stream Nim backend
    /// (selected by `TraceEventsFileFormat::Ctfs`) we opt into
    /// column-aware step encoding right after `begin_writing_trace_events`
    /// and before the first `start` event.  Per the spec, the
    /// column-aware flag is trace-global and must be flipped before
    /// any step is registered.  Other backends silently no-op on
    /// `enable_column_aware_steps` (trait default).
    ///
    /// P1.3 — we also register the activation path together with its
    /// per-line column counts BEFORE `start`.  The Nim
    /// `MultiStreamTraceWriter::registerPath` returns the existing id
    /// without updating the line-length record if the path is already
    /// interned, so the first registration wins.  `start` implicitly
    /// interns the path on its first call, which would lock in an
    /// empty line-length table — defeating the column-aware reader's
    /// `decodeGlobalPositionIndex` round-trip.  Registering the path
    /// here, with the line lengths, before `start` is the cleanest
    /// fix.
    ///
    /// `before_first_record` runs after the writer is opened and before
    /// `start`, the trace's first record. The CTFS writer commits
    /// `meta.dat` at the first record and refuses every `meta.dat`-
    /// affecting call after it (`ctfs-container.md` §6 "Durability"), so
    /// filter provenance and any other `meta.dat` field belong there.
    pub fn configure_writer(
        &self,
        writer: &mut dyn TraceWriter,
        start_path: &Path,
        start_line: u32,
        tables: &mut PathTables,
        before_first_record: impl FnOnce(&mut dyn TraceWriter) -> Result<()>,
    ) -> Result<()> {
        TraceWriter::begin_writing_trace_events(writer, self.events()).map_err(|err| {
            enverr!(ErrorCode::Io, "failed to begin trace events")
                .with_context("path", self.events().display().to_string())
                .with_context("source", err.to_string())
        })?;
        if matches!(self.format, TraceEventsFileFormat::Ctfs) {
            // P1.1: opt the CTFS writer into column-aware step encoding.
            // The opt-in is sticky for the lifetime of the trace and
            // gates the canonical `DeltaColumn` (tag 0x07) emission path
            // exercised by `events.rs::on_line`.
            TraceWriter::enable_column_aware_steps(writer);

            // P1.3 / P6.2: register the activation path with its per-line
            // table, and run the autoformat pass on it, before `start`
            // interns it. Registered through the trace's `PathTables`, so
            // its first step does not register it again.
            tables.register(writer, start_path);
        }
        before_first_record(writer)?;
        TraceWriter::start(writer, start_path, Line(start_line as i64));
        Ok(())
    }
}

/// The file name the CTFS writer gives the container of a recording of
/// `program`: the program's file name without its directory and its last
/// extension, plus `.ct`.
fn ctfs_container_name(program: &str) -> String {
    let stem = Path::new(program)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{stem}.ct")
}

/// Lines in the conventional `paths.dat` Layout A table, registered for a
/// file whose source cannot be read (`internal-files.md` §"`paths.dat`
/// Layout A"). It is the column-aware counterpart of the line-count
/// table's `100000` ceiling.
pub(crate) const CONVENTIONAL_LINE_COUNT: usize = 100_000;

/// Positions per line in the conventional table. A step on such a file
/// whose column exceeds it is recorded at this column.
pub(crate) const CONVENTIONAL_LINE_POSITIONS: u32 = 1024;

/// A file's `paths.dat` Layout A per-line table.
pub(crate) struct SourceLineTable {
    pub line_lengths: Vec<u32>,
    /// The source could not be read and `line_lengths` is the conventional
    /// table, so columns on this file are clamped to
    /// [`CONVENTIONAL_LINE_POSITIONS`].
    pub conventional: bool,
}

/// The `paths.dat` Layout A table for `path`: one entry per source line,
/// the line's **byte length** without its newline. CPython's
/// `co_positions()` `col_offset` is a UTF-8 byte offset into the line, so
/// a byte count keeps the table in the unit of the columns the recorder
/// emits through `write_delta_column`; a character count would shift
/// columns by the number of multi-byte characters before the cursor.
///
/// A file whose lines hold no bytes gets one position on its first line,
/// so an empty file is `[1]`. A file that cannot be read gets the
/// conventional table
/// ([`CONVENTIONAL_LINE_COUNT`] lines of [`CONVENTIONAL_LINE_POSITIONS`]):
/// a Layout A table is never empty, and an empty one would give the file
/// `file_size` 0 (`trace-events.md` §"Per-File Contiguous Integer
/// Ranges").
pub(crate) fn source_line_table(path: &Path) -> SourceLineTable {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut lines: Vec<u32> = Vec::new();
            let mut current_len: u32 = 0;
            for byte in &bytes {
                if *byte == b'\n' {
                    lines.push(current_len);
                    current_len = 0;
                } else {
                    current_len = current_len.saturating_add(1);
                }
            }
            // A file that does not end with a newline still has a final line.
            if current_len > 0 || bytes.last() != Some(&b'\n') {
                lines.push(current_len);
            }
            // A file that holds no bytes on any line (an empty `__init__.py`
            // reads as `[0]`) would have `file_size` 0. Its first line gets
            // one position, so an empty file is `[1]`.
            if lines.iter().all(|&len| len == 0) {
                lines[0] = 1;
            }
            SourceLineTable {
                line_lengths: lines,
                conventional: false,
            }
        }
        Err(_) => SourceLineTable {
            line_lengths: vec![CONVENTIONAL_LINE_POSITIONS; CONVENTIONAL_LINE_COUNT],
            conventional: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codetracer_trace_types::{Line, TraceLowLevelEvent};
    use codetracer_trace_writer_nim::non_streaming_trace_writer::NonStreamingTraceWriter;
    use tempfile::tempdir;

    #[test]
    fn ctfs_paths_name_the_container_after_the_program() {
        let tmp = tempdir().expect("tempdir");
        let paths = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::Ctfs, "/src/app/main.py");
        assert_eq!(paths.events(), tmp.path().join("main.ct").as_path());
    }

    #[test]
    fn binary_paths_use_bin_extension() {
        let tmp = tempdir().expect("tempdir");
        let paths = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::BinaryV0, "program.py");
        assert_eq!(paths.events(), tmp.path().join("trace.bin").as_path());
    }

    #[test]
    fn configure_writer_initialises_writer_state() {
        let tmp = tempdir().expect("tempdir");
        let start_path = tmp.path().join("program.py");
        std::fs::write(&start_path, "print('hi')\n").expect("write script");

        let paths = TraceOutputPaths::new(tmp.path(), TraceEventsFileFormat::BinaryV0, "program.py");
        let mut writer = NonStreamingTraceWriter::new("program.py", &[]);

        paths
            .configure_writer(
                &mut writer,
                &start_path,
                123,
                &mut PathTables::new(false),
                |_| Ok(()),
            )
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
