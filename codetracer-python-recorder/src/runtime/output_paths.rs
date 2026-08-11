//! File-system helpers for trace output management.

use std::path::{Path, PathBuf};

use codetracer_trace_types::{Line, PathId};
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::TraceEventsFileFormat;
use recorder_errors::{enverr, ErrorCode};

use crate::errors::Result;
use crate::runtime::autoformat::{self, AutoformatOutcome, SkipReason};

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
    pub fn new(root: &Path, format: TraceEventsFileFormat) -> Self {
        let events_name = match format {
            TraceEventsFileFormat::Json => "trace.json",
            TraceEventsFileFormat::Ctfs => "trace.ct",
            _ => "trace.bin",
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
        if matches!(self.format, TraceEventsFileFormat::Ctfs) {
            // P1.1: opt the CTFS writer into column-aware step encoding.
            // The opt-in is sticky for the lifetime of the trace and
            // gates the canonical `DeltaColumn` (tag 0x07) emission path
            // exercised by `events.rs::on_line`.
            TraceWriter::enable_column_aware_steps(writer);

            // P1.3: register the activation path with its per-line
            // column counts *before* `start` so the paths.dat record
            // carries the Layout A line-length table the reader needs
            // for column resolution.  `register_path_with_line_lengths`
            // is best-effort: if the file isn't readable (rare for the
            // activation path) we fall back to an empty slice and the
            // reader surfaces column = 1 for steps on this file.
            let line_lengths = read_line_lengths_for_path(start_path);
            match TraceWriter::register_path_with_line_lengths(writer, start_path, &line_lengths) {
                Ok(start_path_id) => {
                    // P6.2: the activation path's first registration is
                    // also the canonical hook point for the recorder-
                    // side autoformat pass.  This catches minified
                    // *entrypoints* — programs whose top-level file is
                    // itself a packed bundle.  Steady-state hand-written
                    // entrypoints land on
                    // [`SkipReason::NotMinified`] inside the helper and
                    // the call is essentially free.  See
                    // [`maybe_register_autoformat_view_for_path`] for
                    // the full decision tree.
                    maybe_register_autoformat_view_for_path(writer, start_path_id, start_path);
                }
                Err(err) => {
                    log::debug!(
                        "[TraceOutputPaths] register_path_with_line_lengths failed for {}: {} \
                         (column resolution will fall back to None for this file)",
                        start_path.display(),
                        err,
                    );
                }
            }
        }
        TraceWriter::start(writer, start_path, Line(start_line as i64));
        Ok(())
    }
}

/// P6.2: run the recorder-side autoformat pass on `source_path` and,
/// on a successful outcome, buffer a ``black``-formatted view of the
/// source into the CTFS writer's ``source_views.dat`` stream via
/// [`TraceWriter::register_source_view`].
///
/// This is the activation-path counterpart of
/// ``events.rs::maybe_register_autoformat_view`` — see that function's
/// docstring for the full decision tree and naming conventions.  The
/// only structural difference: this helper is called from
/// `configure_writer` *before* any step events are emitted, so it
/// covers the entrypoint script itself (a minified `bundle.py` invoked
/// directly).  Steady-state hand-written entrypoints land on
/// [`SkipReason::NotMinified`] inside `try_autoformat` and never make
/// it past the heuristic.
fn maybe_register_autoformat_view_for_path(
    writer: &mut dyn TraceWriter,
    path_id: PathId,
    source_path: &Path,
) {
    let lossy = source_path.to_string_lossy();
    if lossy.starts_with('<') && lossy.ends_with('>') {
        return;
    }
    let content = match std::fs::read_to_string(source_path) {
        Ok(s) => s,
        Err(_) => return,
    };
    match autoformat::try_autoformat(&content, source_path) {
        AutoformatOutcome::Ok(result) => {
            let stem = source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("source");
            let view_name = format!("{stem}.fmt.py");
            // Spec ``view_kind = 2`` = ``black_format``.
            const VIEW_KIND_BLACK_FORMAT: u8 = 2;
            if let Err(err) = TraceWriter::register_source_view(
                writer,
                path_id,
                VIEW_KIND_BLACK_FORMAT,
                &view_name,
                result.formatted_content.as_bytes(),
                result.sourcemap_v3_json.as_bytes(),
            ) {
                log::error!(
                    "[TraceOutputPaths] register_source_view failed for {}: {} \
                     (formatted view will not appear in the trace)",
                    source_path.display(),
                    err,
                );
            }
        }
        AutoformatOutcome::Skipped(reason) => match reason {
            SkipReason::ToolMissing => {
                log::debug!(
                    "[TraceOutputPaths] autoformat skipped for {}: black not on PATH \
                     (formatted view will not appear in the trace)",
                    source_path.display(),
                );
            }
            SkipReason::ToolError(msg) => {
                log::debug!(
                    "[TraceOutputPaths] autoformat skipped for {}: black error: {} \
                     (formatted view will not appear in the trace)",
                    source_path.display(),
                    msg,
                );
            }
            SkipReason::NotMinified
            | SkipReason::EnvDisabled
            | SkipReason::SiblingMapExists
            | SkipReason::NoChange => {
                // Steady-state: don't log.
            }
        },
    }
}

/// P1.3: read `path` and compute the per-line column counts used as
/// the `paths.dat` Layout A `line_lengths` table.  Mirrors the
/// `read_line_lengths` helper in `events.rs` — duplicated here so the
/// `configure_writer` hot path doesn't have to dip into the tracer-
/// events module.  Returns an empty Vec when the file isn't readable
/// or is a synthetic Python path (`<...>`).
///
/// Each entry is the byte length of the source line (excluding the
/// trailing newline), matching CPython's `co_positions()` `col_offset`
/// reporting convention.  See `events.rs::read_line_lengths` for the
/// full rationale.
fn read_line_lengths_for_path(path: &Path) -> Vec<u32> {
    let lossy = path.to_string_lossy();
    if lossy.starts_with('<') && lossy.ends_with('>') {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
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
    if current_len > 0 || bytes.last() != Some(&b'\n') {
        lines.push(current_len);
    }
    lines
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
